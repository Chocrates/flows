import { describe, expect, it } from "vitest";
import { moveCard, patchCard } from "../src/client/actions";
import { UndoStack } from "../src/client/history";
import { BoardStore } from "../src/client/store";
import type { BoardSnapshot, CardRecord } from "../src/shared/types";

function card(over: Partial<CardRecord> = {}): CardRecord {
  return {
    id: "c1", boardId: "b1", columnId: "todo", title: "Write tests", description: "", rank: "m",
    labels: [], assignees: [], dueAt: null, archived: false, version: 1,
    updatedAt: "2024-01-01T00:00:00Z", updatedBy: "u1", ...over,
  };
}

function setup(clock: { t: number }) {
  const store = new BoardStore();
  const undo = new UndoStack(() => clock.t);
  store.use(undo.middleware());
  const snapshot: BoardSnapshot = {
    board: { id: "b1", name: "B", ownerId: "u1", members: [], labels: [], version: 1, createdAt: "" },
    columns: {
      todo: { id: "todo", boardId: "b1", name: "To do", rank: "a", wipLimit: null, version: 1 },
      done: { id: "done", boardId: "b1", name: "Done", rank: "b", wipLimit: null, version: 1 },
    },
    cards: { c1: card() },
  };
  store.dispatch({ type: "board/loaded", snapshot });
  return { store, undo };
}

describe("undo stack", () => {
  it("does not record the initial load", () => {
    const { undo } = setup({ t: 0 });
    expect(undo.canUndo()).toBe(false);
  });

  it("undoes a rename", () => {
    const clock = { t: 0 };
    const { store, undo } = setup(clock);
    store.dispatch(patchCard(store.getState().cards.c1, { title: "Write more tests" }));
    undo.undo((a) => store.dispatch(a));
    expect(store.getState().cards.c1.title).toBe("Write tests");
    expect(undo.canRedo()).toBe(true);
  });

  it("restores the original column after a move", () => {
    const clock = { t: 0 };
    const { store, undo } = setup(clock);
    store.dispatch(moveCard(store.getState().cards.c1, "done", "n"));
    undo.undo((a) => store.dispatch(a));
    expect(store.getState().cards.c1.columnId).toBe("todo");
  });

  it("new edits clear the redo branch", () => {
    const clock = { t: 0 };
    const { store, undo } = setup(clock);
    store.dispatch(patchCard(store.getState().cards.c1, { title: "A" }));
    undo.undo((a) => store.dispatch(a));
    clock.t = 5000;
    store.dispatch(patchCard(store.getState().cards.c1, { title: "B" }));
    expect(undo.canRedo()).toBe(false);
  });
});
