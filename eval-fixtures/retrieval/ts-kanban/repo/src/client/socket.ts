/**
 * Realtime channel. Receives server events for the open board and folds them
 * into the store via the merge rules in merge.ts.
 */
import type { ServerEvent, Id } from "../shared/types";
import type { BoardStore } from "./store";
import { reconcileCard } from "./merge";

export interface ChannelOptions {
  url: string;
  boardId: Id;
  clientId: string;
  token: () => string | null;
  /** Called once the socket (re)opens so the caller can refetch missed state. */
  onResync?: () => void;
}

const HEARTBEAT_MS = 25_000;

export class LiveChannel {
  private ws: WebSocket | null = null;
  private heartbeat: ReturnType<typeof setInterval> | null = null;
  private closedByUs = false;
  private attempt = 0;

  constructor(private opts: ChannelOptions, private store: BoardStore) {}

  open(): void {
    this.closedByUs = false;
    const url = `${this.opts.url}?board=${encodeURIComponent(this.opts.boardId)}&token=${this.opts.token() ?? ""}`;
    const ws = new WebSocket(url);
    this.ws = ws;

    ws.onopen = () => {
      this.attempt = 0;
      this.heartbeat = setInterval(() => ws.send('{"kind":"ping"}'), HEARTBEAT_MS);
      this.opts.onResync?.();
    };
    ws.onmessage = (ev) => this.handle(JSON.parse(String(ev.data)) as ServerEvent);
    ws.onclose = () => {
      if (this.heartbeat) clearInterval(this.heartbeat);
      this.heartbeat = null;
      if (!this.closedByUs && this.attempt < 3) {
        const delay = 1000 * 2 ** this.attempt++;
        setTimeout(() => this.open(), delay);
      }
    };
  }

  close(): void {
    this.closedByUs = true;
    this.ws?.close();
  }

  private handle(ev: ServerEvent): void {
    const state = this.store.getState();
    switch (ev.kind) {
      case "card.upserted": {
        const merged = reconcileCard(state.cards[ev.card.id], ev.card);
        this.store.dispatch({ type: "remote/applied", cards: [merged], columns: [], removedCards: [] });
        break;
      }
      case "card.removed":
        this.store.dispatch({ type: "remote/applied", cards: [], columns: [], removedCards: [ev.cardId] });
        break;
      case "column.upserted":
        this.store.dispatch({ type: "remote/applied", cards: [], columns: [ev.column], removedCards: [] });
        break;
      case "column.removed":
        // TODO: not handled; the column lingers until reload.
        break;
      case "member.changed":
        break;
    }
  }
}
