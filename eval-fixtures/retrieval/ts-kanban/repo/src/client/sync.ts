/**
 * Optimistic write path. Local actions are applied to the store immediately
 * and queued in the outbox; a single drain loop pushes them to the server in
 * order. On rejection the local change is rolled back.
 */
import type { CardRecord } from "../shared/types";
import type { BoardAction } from "./actions";
import { invert, isRemote } from "./actions";
import { ApiError, BoardApi } from "./api";
import { Outbox, PendingOp, QueuedOp } from "./outbox";
import type { BoardStore, Middleware } from "./store";

export interface SyncEvents {
  onConflict(local: CardRecord, server: CardRecord): void;
  onOfflineChange(offline: boolean): void;
  onError(err: unknown): void;
}

export class SyncEngine {
  private draining = false;
  private offline = !navigator.onLine;
  private inverses = new Map<number, BoardAction>();

  constructor(
    private store: BoardStore,
    private api: BoardApi,
    private outbox: Outbox,
    private events: SyncEvents,
  ) {
    window.addEventListener("online", () => this.setOffline(false));
    window.addEventListener("offline", () => this.setOffline(true));
  }

  middleware(): Middleware {
    return (action, next, getState) => {
      next(action);
      if (isRemote(action)) return;
      const op = this.toOp(action, getState().cards);
      if (!op) return;
      const queued = this.outbox.enqueue(op);
      const inv = invert(action);
      if (inv) this.inverses.set(queued.seq, inv);
      void this.drain();
    };
  }

  private toOp(action: BoardAction, cards: Record<string, CardRecord>): PendingOp | null {
    switch (action.type) {
      case "card/added":
        return { op: "create", card: action.card };
      case "card/patched":
        return { op: "update", cardId: action.cardId, baseVersion: cards[action.cardId]?.version ?? 0, patch: action.patch };
      case "card/moved":
        return { op: "move", cardId: action.cardId, baseVersion: cards[action.cardId]?.version ?? 0, columnId: action.toColumn, rank: action.rank };
      case "card/removed":
        return { op: "delete", cardId: action.card.id };
      default:
        return null;
    }
  }

  private setOffline(offline: boolean): void {
    this.offline = offline;
    this.events.onOfflineChange(offline);
  }

  async drain(): Promise<void> {
    if (this.draining || this.offline) return;
    this.draining = true;
    try {
      for (let item = this.outbox.peek(); item; item = this.outbox.peek()) {
        const ok = await this.send(item);
        if (!ok) break;
      }
    } finally {
      this.draining = false;
    }
  }

  private async send(item: QueuedOp): Promise<boolean> {
    try {
      const saved = await this.dispatchOp(item.body);
      this.outbox.ack(item.seq);
      this.inverses.delete(item.seq);
      if (saved) this.store.dispatch({ type: "remote/applied", cards: [saved], columns: [], removedCards: [] });
      return true;
    } catch (err) {
      if (err instanceof ApiError && err.isConflict && item.body.op !== "create" && item.body.op !== "delete") {
        const server = err.body?.current as CardRecord;
        const local = this.store.getState().cards[item.body.cardId];
        this.outbox.ack(item.seq);
        this.events.onConflict(local, server);
        return true;
      }
      if (err instanceof ApiError && err.status >= 400 && err.status < 500) {
        this.rollback(item);
        this.events.onError(err);
        return true;
      }
      // Network failure after retries: leave it queued and stop draining.
      this.outbox.bumpAttempts(item.seq);
      this.setOffline(true);
      return false;
    }
  }

  private rollback(item: QueuedOp): void {
    this.outbox.ack(item.seq);
    const inv = this.inverses.get(item.seq);
    this.inverses.delete(item.seq);
    if (inv && inv.type !== "card/moved") this.store.dispatch(inv);
  }

  private async dispatchOp(op: PendingOp): Promise<CardRecord | null> {
    switch (op.op) {
      case "create":
        return this.api.createCard(op.card);
      case "update":
        return this.api.updateCard(op.cardId, op.baseVersion, op.patch);
      case "move":
        return this.api.moveCard(op.cardId, op.baseVersion, op.columnId, op.rank);
      case "delete":
        await this.api.deleteCard(op.cardId);
        return null;
    }
  }
}
