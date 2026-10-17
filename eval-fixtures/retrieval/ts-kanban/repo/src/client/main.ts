/**
 * Browser entry point: wires the store, middleware, network layers and
 * input handlers together for one open board.
 */
import type { BoardMember, Id } from "../shared/types";
import { can } from "../shared/acl";
import { BoardApi } from "./api";
import { DragController, sortedColumn } from "./dnd";
import { UndoStack } from "./history";
import { installShortcuts } from "./hotkeys";
import { Outbox } from "./outbox";
import { ReminderScheduler } from "./reminders";
import { LiveChannel } from "./socket";
import { BoardStore } from "./store";
import { SyncEngine } from "./sync";

export interface BootOptions {
  apiBase: string;
  liveUrl: string;
  boardId: Id;
  me: BoardMember;
  token: () => string | null;
  root: HTMLElement;
  notify: (msg: string) => void;
}

export async function boot(opts: BootOptions): Promise<() => void> {
  const clientId = crypto.randomUUID();
  const store = new BoardStore();
  const api = new BoardApi(opts.apiBase, opts.token, clientId);
  const undo = new UndoStack();
  const outbox = new Outbox(opts.boardId);
  const sync = new SyncEngine(store, api, outbox, {
    onConflict: () => opts.notify("sync.conflict"),
    onOfflineChange: (off) => off && opts.notify("sync.offline"),
    onError: () => opts.notify("sync.failed"),
  });

  // Order matters: history records first, then sync queues the write.
  store.use(undo.middleware());
  store.use(sync.middleware());

  store.dispatch({ type: "board/loaded", snapshot: await api.loadBoard(opts.boardId) });

  const live = new LiveChannel(
    { url: opts.liveUrl, boardId: opts.boardId, clientId, token: opts.token, onResync: () => void sync.drain() },
    store,
  );
  live.open();

  const reminders = new ReminderScheduler(opts.me);
  store.subscribe((s) => s.cards, (cards) => reminders.sync(Object.values(cards)));

  const drag = new DragController(opts.root, store, () => can(opts.me.role, "card.move"));

  const removeKeys = installShortcuts(window, (cmd) => {
    if (cmd === "undo") undo.undo((a) => store.dispatch(a));
    else if (cmd === "redo") undo.redo((a) => store.dispatch(a));
    else opts.root.dispatchEvent(new CustomEvent("board-command", { detail: cmd }));
  });

  // Expose for the column view.
  (window as unknown as Record<string, unknown>).__columnCards = (col: Id) => sortedColumn(store.getState().cards, col);

  return () => {
    live.close();
    drag.destroy();
    removeKeys();
  };
}
