/**
 * Pure reducer over the normalised board snapshot. No I/O here: network
 * effects live in sync.ts, which observes dispatched actions.
 */
import type { BoardSnapshot, CardRecord } from "../shared/types";
import type { BoardAction } from "./actions";

export const emptySnapshot = (): BoardSnapshot => ({
  board: { id: "", name: "", ownerId: "", members: [], labels: [], version: 0, createdAt: "" },
  columns: {},
  cards: {},
});

export function boardReducer(state: BoardSnapshot, action: BoardAction): BoardSnapshot {
  switch (action.type) {
    case "board/loaded":
      return action.snapshot;

    case "card/added":
      return { ...state, cards: { ...state.cards, [action.card.id]: action.card } };

    case "card/patched": {
      const card = state.cards[action.cardId];
      if (!card) return state;
      return { ...state, cards: { ...state.cards, [card.id]: { ...card, ...action.patch } } };
    }

    case "card/moved": {
      const card = state.cards[action.cardId];
      if (!card || !state.columns[action.toColumn]) return state;
      const moved: CardRecord = { ...card, columnId: action.toColumn, rank: action.rank };
      return { ...state, cards: { ...state.cards, [card.id]: moved } };
    }

    case "card/removed": {
      const { [action.card.id]: _gone, ...rest } = state.cards;
      return { ...state, cards: rest };
    }

    case "column/added":
      return { ...state, columns: { ...state.columns, [action.column.id]: action.column } };

    case "column/renamed": {
      const col = state.columns[action.columnId];
      if (!col) return state;
      return { ...state, columns: { ...state.columns, [col.id]: { ...col, name: action.name } } };
    }

    case "column/removed": {
      // Cards in the removed column are dropped along with it.
      const { [action.column.id]: _col, ...columns } = state.columns;
      const cards: Record<string, CardRecord> = {};
      for (const c of Object.values(state.cards)) {
        if (c.columnId !== action.column.id) cards[c.id] = c;
      }
      return { ...state, columns, cards };
    }

    case "remote/applied": {
      const cards = { ...state.cards };
      for (const c of action.cards) cards[c.id] = c;
      for (const id of action.removedCards) delete cards[id];
      const columns = { ...state.columns };
      for (const col of action.columns) columns[col.id] = col;
      return { ...state, cards, columns };
    }
  }
}
