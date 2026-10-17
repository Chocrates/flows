/**
 * Board, column and membership endpoints.
 */
import { Router } from "express";
import { canAssignRole } from "../../shared/acl";
import { AuthedRequest, requireCapability, requireMember } from "../auth";
import type { BoardHub } from "../broadcast";
import type { BoardDao, CardDao } from "../dao";
import { checkRole } from "../schemas";

export function boardRouter(boards: BoardDao, cards: CardDao, hub: BoardHub): Router {
  const r = Router();
  const boardOfColumn = (req: AuthedRequest) => boards.findColumn(req.params.id)?.boardId ?? null;

  r.get("/boards/:boardId", requireMember(boards, (req) => req.params.boardId), (req, res) => {
    const ar = req as unknown as AuthedRequest;
    const board = ar.board!;
    res.json({
      board,
      columns: Object.fromEntries(boards.columnsOf(board.id).map((c) => [c.id, c])),
      cards: Object.fromEntries(cards.listByBoard(board.id).map((c) => [c.id, c])),
    });
  });

  r.patch("/columns/:id", requireMember(boards, boardOfColumn), (req, res) => {
    const ar = req as unknown as AuthedRequest;
    const name = typeof req.body?.name === "string" ? req.body.name.trim() : "";
    if (!name) return void res.status(400).json({ code: "validation", message: "name required" });
    const col = boards.renameColumn(req.params.id, name);
    hub.publish(col.boardId, { kind: "column.upserted", column: col, originClientId: ar.clientId });
    res.json(col);
  });

  r.delete("/columns/:id", requireCapability(boards, "column.delete", boardOfColumn), (req, res) => {
    const ar = req as unknown as AuthedRequest;
    const col = boards.findColumn(req.params.id)!;
    boards.deleteColumn(col.id);
    hub.publish(col.boardId, { kind: "column.removed", columnId: col.id, boardId: col.boardId, originClientId: ar.clientId });
    res.status(204).end();
  });

  r.put(
    "/boards/:boardId/members/:userId/role",
    requireCapability(boards, "board.changeRole", (req) => req.params.boardId),
    (req, res) => {
      const ar = req as unknown as AuthedRequest;
      const next = req.body?.role;
      if (!checkRole(next)) return void res.status(400).json({ code: "validation", message: "unknown role" });
      const target = ar.board!.members.find((m) => m.userId === req.params.userId);
      if (!target) return void res.status(404).json({ code: "not_found", message: "not a member" });
      if (!canAssignRole(ar.role!, target.role, next)) {
        return void res.status(403).json({ code: "forbidden", message: "cannot assign that role" });
      }
      boards.setRole(ar.board!.id, target.userId, next);
      hub.publish(ar.board!.id, { kind: "member.changed", boardId: ar.board!.id, member: { ...target, role: next } });
      if (next === "viewer") hub.evict(ar.board!.id, target.userId);
      res.status(204).end();
    },
  );

  return r;
}
