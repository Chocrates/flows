/**
 * /boards/:boardId/cards and /cards/:id endpoints.
 */
import { Router, Response } from "express";
import type { CardRecord } from "../../shared/types";
import { needsRebalance, spreadRanks, compareRank } from "../../shared/ordering";
import { AuthedRequest, requireCapability } from "../auth";
import type { BoardHub } from "../broadcast";
import { BoardDao, CardDao, StaleWriteError } from "../dao";
import { checkCardPatch, checkMove, checkNewCard } from "../schemas";

export function cardRouter(boards: BoardDao, cards: CardDao, hub: BoardHub): Router {
  const r = Router();
  const boardOfCard = (req: AuthedRequest) => cards.find(req.params.id)?.boardId ?? null;

  r.post(
    "/boards/:boardId/cards",
    requireCapability(boards, "card.create", (req) => req.params.boardId),
    (req, res) => {
      const ar = req as unknown as AuthedRequest;
      const checked = checkNewCard(req.body);
      if (!checked.ok) return void res.status(400).json({ code: "validation", message: "invalid card", details: checked.errors });
      const v = checked.value;
      const card: CardRecord = {
        id: v.id,
        boardId: ar.board!.id,
        columnId: v.columnId,
        title: v.title,
        description: v.description ?? "",
        rank: v.rank,
        labels: [],
        assignees: [],
        dueAt: v.dueAt ?? null,
        archived: false,
        version: 1,
        updatedAt: new Date().toISOString(),
        updatedBy: ar.userId,
      };
      const saved = cards.insert(card);
      hub.publish(saved.boardId, { kind: "card.upserted", card: saved, originClientId: ar.clientId });
      res.status(201).json(saved);
    },
  );

  r.patch("/cards/:id", requireCapability(boards, "card.edit", boardOfCard), (req, res) => {
    const ar = req as unknown as AuthedRequest;
    const checked = checkCardPatch(req.body);
    if (!checked.ok) return void res.status(400).json({ code: "validation", message: "invalid patch", details: checked.errors });
    write(res, ar, () => cards.update(req.params.id, checked.value.baseVersion, checked.value.patch, ar.userId));
  });

  r.post("/cards/:id/move", requireCapability(boards, "card.move", boardOfCard), (req, res) => {
    const ar = req as unknown as AuthedRequest;
    const checked = checkMove(req.body);
    if (!checked.ok) return void res.status(400).json({ code: "validation", message: "invalid move", details: checked.errors });
    const { baseVersion, columnId, rank } = checked.value;
    write(res, ar, () => {
      const moved = cards.update(req.params.id, baseVersion, { columnId, rank }, ar.userId);
      const siblings = cards.listByBoard(moved.boardId).filter((c) => c.columnId === columnId).sort(compareRank);
      if (needsRebalance(siblings.map((c) => c.rank))) {
        const fresh = spreadRanks(siblings.length);
        cards.rewriteRanks(columnId, new Map(siblings.map((c, i) => [c.id, fresh[i]])));
      }
      return cards.find(moved.id)!;
    });
  });

  r.delete("/cards/:id", requireCapability(boards, "card.delete", boardOfCard), (req, res) => {
    const ar = req as unknown as AuthedRequest;
    const card = cards.find(req.params.id)!;
    cards.remove(card.id);
    hub.publish(card.boardId, { kind: "card.removed", cardId: card.id, boardId: card.boardId, originClientId: ar.clientId });
    res.status(204).end();
  });

  function write(res: Response, ar: AuthedRequest, fn: () => CardRecord): void {
    try {
      const saved = fn();
      hub.publish(saved.boardId, { kind: "card.upserted", card: saved, originClientId: ar.clientId });
      res.json(saved);
    } catch (err) {
      if (err instanceof StaleWriteError) {
        res.status(409).json({ code: "conflict", message: "card changed", current: err.current });
        return;
      }
      throw err;
    }
  }

  return r;
}
