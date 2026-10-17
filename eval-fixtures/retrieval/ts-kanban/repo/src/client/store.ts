/**
 * Minimal observable container. Views call `subscribe` with a selector and
 * are only re-run when the selected slice changes by reference.
 */
import type { BoardSnapshot } from "../shared/types";
import type { BoardAction } from "./actions";
import { boardReducer, emptySnapshot } from "./reducer";

export type Listener<T> = (value: T, prev: T) => void;
export type Middleware = (action: BoardAction, next: (a: BoardAction) => void, getState: () => BoardSnapshot) => void;

interface Subscription<T> {
  select: (s: BoardSnapshot) => T;
  listener: Listener<T>;
  last: T;
}

export class BoardStore {
  private state: BoardSnapshot = emptySnapshot();
  private subs = new Set<Subscription<unknown>>();
  private middleware: Middleware[] = [];
  private dispatching = false;
  private queued: BoardAction[] = [];

  getState(): BoardSnapshot {
    return this.state;
  }

  use(mw: Middleware): void {
    this.middleware.push(mw);
  }

  /**
   * Dispatch through middleware, then reduce. Re-entrant dispatches from
   * listeners are queued and flushed after the current notify pass.
   */
  dispatch(action: BoardAction): void {
    if (this.dispatching) {
      this.queued.push(action);
      return;
    }
    const chain = this.middleware.reduceRight<(a: BoardAction) => void>(
      (next, mw) => (a) => mw(a, next, () => this.state),
      (a) => this.commit(a),
    );
    chain(action);
  }

  private commit(action: BoardAction): void {
    this.dispatching = true;
    try {
      this.state = boardReducer(this.state, action);
      this.notify();
    } finally {
      this.dispatching = false;
    }
    const pending = this.queued.splice(0);
    for (const a of pending) this.dispatch(a);
  }

  subscribe<T>(select: (s: BoardSnapshot) => T, listener: Listener<T>): () => void {
    const sub: Subscription<T> = { select, listener, last: select(this.state) };
    this.subs.add(sub as Subscription<unknown>);
    return () => this.subs.delete(sub as Subscription<unknown>);
  }

  private notify(): void {
    for (const sub of this.subs) {
      const next = sub.select(this.state);
      if (next !== sub.last) {
        const prev = sub.last;
        sub.last = next;
        sub.listener(next, prev);
      }
    }
  }
}
