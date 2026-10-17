/**
 * Action shapes understood by the board reducer. Every action that mutates
 * board data carries enough information to be inverted by the undo stack.
 */
import type { CardRecord, ColumnRecord, Id, BoardSnapshot } from "../shared/types";

export type BoardAction =
  | { type: "board/loaded"; snapshot: BoardSnapshot }
  | { type: "card/added"; card: CardRecord }
  | { type: "card/patched"; cardId: Id; patch: Partial<CardRecord>; prev: Partial<CardRecord> }
  | { type: "card/moved"; cardId: Id; toColumn: Id; rank: string; fromColumn: Id; prevRank: string }
  | { type: "card/removed"; card: CardRecord }
  | { type: "column/added"; column: ColumnRecord }
  | { type: "column/renamed"; columnId: Id; name: string; prevName: string }
  | { type: "column/removed"; column: ColumnRecord; cards: CardRecord[] }
  | { type: "remote/applied"; cards: CardRecord[]; columns: ColumnRecord[]; removedCards: Id[] };

/** Actions that originate from the server and must never enter undo history. */
export function isRemote(action: BoardAction): boolean {
  return action.type === "board/loaded" || action.type === "remote/applied";
}

export const moveCard = (
  card: CardRecord,
  toColumn: Id,
  rank: string,
): BoardAction => ({
  type: "card/moved",
  cardId: card.id,
  toColumn,
  rank,
  fromColumn: card.columnId,
  prevRank: card.rank,
});

export const patchCard = (card: CardRecord, patch: Partial<CardRecord>): BoardAction => {
  const prev: Partial<CardRecord> = {};
  for (const k of Object.keys(patch) as (keyof CardRecord)[]) {
    (prev as Record<string, unknown>)[k] = card[k];
  }
  return { type: "card/patched", cardId: card.id, patch, prev };
};

export const renameColumn = (column: ColumnRecord, name: string): BoardAction => ({
  type: "column/renamed",
  columnId: column.id,
  name,
  prevName: column.name,
});

/** Build the action that undoes `action`, or null if it is not invertible. */
export function invert(action: BoardAction): BoardAction | null {
  switch (action.type) {
    case "card/added":
      return { type: "card/removed", card: action.card };
    case "card/removed":
      return { type: "card/added", card: action.card };
    case "card/patched":
      return { type: "card/patched", cardId: action.cardId, patch: action.prev, prev: action.patch };
    case "card/moved":
      return {
        type: "card/moved",
        cardId: action.cardId,
        toColumn: action.fromColumn,
        rank: action.prevRank,
        fromColumn: action.toColumn,
        prevRank: action.rank,
      };
    case "column/renamed":
      return { type: "column/renamed", columnId: action.columnId, name: action.prevName, prevName: action.name };
    case "column/added":
      return { type: "column/removed", column: action.column, cards: [] };
    case "column/removed":
      return { type: "column/added", column: action.column };
    default:
      return null;
  }
}
