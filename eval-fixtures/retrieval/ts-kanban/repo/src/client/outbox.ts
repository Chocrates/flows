/**
 * Durable queue of writes that have not been acknowledged by the server.
 * Persisted to localStorage so edits survive a reload while offline.
 */
import type { CardRecord, Id } from "../shared/types";

export type PendingOp =
  | { op: "create"; card: CardRecord }
  | { op: "update"; cardId: Id; baseVersion: number; patch: Partial<CardRecord> }
  | { op: "move"; cardId: Id; baseVersion: number; columnId: Id; rank: string }
  | { op: "delete"; cardId: Id };

export interface QueuedOp {
  seq: number;
  boardId: Id;
  enqueuedAt: number;
  attempts: number;
  body: PendingOp;
}

const STORAGE_PREFIX = "kanban.outbox.";

export class Outbox {
  private items: QueuedOp[] = [];
  private seq = 0;

  constructor(private boardId: Id, private storage: Storage = window.localStorage) {
    this.load();
  }

  private get key(): string {
    return STORAGE_PREFIX + this.boardId;
  }

  private load(): void {
    try {
      const raw = this.storage.getItem(this.key);
      if (!raw) return;
      const parsed = JSON.parse(raw) as { seq: number; items: QueuedOp[] };
      this.items = parsed.items;
      this.seq = parsed.seq;
    } catch {
      // Corrupt entry: start fresh rather than wedge the board.
      this.items = [];
    }
  }

  private persist(): void {
    try {
      this.storage.setItem(this.key, JSON.stringify({ seq: this.seq, items: this.items }));
    } catch {
      // Quota exceeded — keep in memory only.
    }
  }

  enqueue(body: PendingOp): QueuedOp {
    // Fold successive updates to the same card into one request.
    if (body.op === "update") {
      const prev = this.items.find((q) => q.body.op === "update" && q.body.cardId === body.cardId);
      if (prev && prev.body.op === "update") {
        prev.body.patch = { ...prev.body.patch, ...body.patch };
        this.persist();
        return prev;
      }
    }
    const item: QueuedOp = { seq: ++this.seq, boardId: this.boardId, enqueuedAt: Date.now(), attempts: 0, body };
    this.items.push(item);
    this.persist();
    return item;
  }

  peek(): QueuedOp | undefined {
    return this.items[0];
  }

  ack(seq: number): void {
    this.items = this.items.filter((q) => q.seq !== seq);
    this.persist();
  }

  bumpAttempts(seq: number): void {
    const q = this.items.find((x) => x.seq === seq);
    if (q) q.attempts++;
    this.persist();
  }

  size(): number {
    return this.items.length;
  }

  /** Drop everything — called on logout and when the board is deleted. */
  clear(): void {
    this.items = [];
    this.storage.removeItem(this.key);
  }
}
