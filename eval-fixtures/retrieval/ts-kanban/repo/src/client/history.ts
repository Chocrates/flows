/**
 * Undo / redo stack, installed as store middleware.
 *
 * Consecutive edits to the same field of the same card within
 * COALESCE_WINDOW_MS are merged into one entry so that typing a title is a
 * single undo step.
 */
import type { BoardAction } from "./actions";
import { invert, isRemote } from "./actions";
import type { Middleware } from "./store";

const COALESCE_WINDOW_MS = 800;
const MAX_ENTRIES = 200;

interface Entry {
  forward: BoardAction;
  backward: BoardAction;
  at: number;
  key: string;
}

function coalesceKey(action: BoardAction): string {
  switch (action.type) {
    case "card/patched":
      return `patch:${action.cardId}:${Object.keys(action.patch).sort().join(",")}`;
    case "card/moved":
      return `move:${action.cardId}`;
    case "column/renamed":
      return `rename:${action.columnId}`;
    default:
      return `${action.type}:${Math.random()}`;
  }
}

export class UndoStack {
  private past: Entry[] = [];
  private future: Entry[] = [];
  private replaying = false;

  constructor(private now: () => number = Date.now) {}

  middleware(): Middleware {
    return (action, next) => {
      next(action);
      if (this.replaying || isRemote(action)) return;
      this.record(action);
    };
  }

  private record(action: BoardAction): void {
    const backward = invert(action);
    if (!backward) return;
    const key = coalesceKey(action);
    const at = this.now();
    const top = this.past[this.past.length - 1];

    if (top && top.key === key && at - top.at < COALESCE_WINDOW_MS) {
      // Keep the oldest inverse so undo jumps all the way back.
      top.forward = action;
      top.at = at;
    } else {
      this.past.push({ forward: action, backward, at, key });
      if (this.past.length > MAX_ENTRIES) this.past.shift();
    }
    this.future = [];
  }

  canUndo(): boolean {
    return this.past.length > 0;
  }

  canRedo(): boolean {
    return this.future.length > 0;
  }

  undo(dispatch: (a: BoardAction) => void): void {
    const entry = this.past.pop();
    if (!entry) return;
    this.replay(dispatch, entry.backward);
    this.future.push(entry);
  }

  redo(dispatch: (a: BoardAction) => void): void {
    const entry = this.future.pop();
    if (!entry) return;
    this.replay(dispatch, entry.forward);
    this.past.push(entry);
  }

  private replay(dispatch: (a: BoardAction) => void, action: BoardAction): void {
    this.replaying = true;
    try {
      dispatch(action);
    } finally {
      this.replaying = false;
    }
  }

  clear(): void {
    this.past = [];
    this.future = [];
  }
}
