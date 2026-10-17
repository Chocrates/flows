/**
 * Pointer-driven reordering of cards within and across columns.
 * Computes the drop slot from the pointer position and turns it into a
 * rank key between the neighbouring cards.
 */
import type { CardRecord, Id } from "../shared/types";
import { compareRank, rankBetween } from "../shared/ordering";
import { moveCard } from "./actions";
import type { BoardStore } from "./store";

interface DragState {
  cardId: Id;
  originColumn: Id;
  ghost: HTMLElement;
  offsetY: number;
}

export function sortedColumn(cards: Record<Id, CardRecord>, columnId: Id): CardRecord[] {
  return Object.values(cards)
    .filter((c) => c.columnId === columnId && !c.archived)
    .sort(compareRank);
}

/** Index in `list` at which a card dropped at `clientY` should land. */
export function dropIndex(listEl: HTMLElement, clientY: number, draggingId: Id): number {
  const items = Array.from(listEl.querySelectorAll<HTMLElement>("[data-card-id]")).filter(
    (el) => el.dataset.cardId !== draggingId,
  );
  for (let i = 0; i < items.length; i++) {
    const r = items[i].getBoundingClientRect();
    if (clientY < r.top + r.height / 2) return i;
  }
  return items.length;
}

export class DragController {
  private drag: DragState | null = null;

  constructor(private root: HTMLElement, private store: BoardStore, private canMove: () => boolean) {
    root.addEventListener("pointerdown", this.onDown);
  }

  private onDown = (e: PointerEvent): void => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-card-id]");
    if (!el || !this.canMove() || e.button !== 0) return;
    const rect = el.getBoundingClientRect();
    const ghost = el.cloneNode(true) as HTMLElement;
    ghost.classList.add("card--ghost");
    document.body.appendChild(ghost);
    this.drag = {
      cardId: el.dataset.cardId!,
      originColumn: el.closest<HTMLElement>("[data-column-id]")!.dataset.columnId!,
      ghost,
      offsetY: e.clientY - rect.top,
    };
    window.addEventListener("pointermove", this.onMove);
    window.addEventListener("pointerup", this.onUp, { once: true });
  };

  private onMove = (e: PointerEvent): void => {
    if (!this.drag) return;
    this.drag.ghost.style.transform = `translate(${e.clientX}px, ${e.clientY - this.drag.offsetY}px)`;
  };

  private onUp = (e: PointerEvent): void => {
    window.removeEventListener("pointermove", this.onMove);
    const drag = this.drag;
    this.drag = null;
    if (!drag) return;
    drag.ghost.remove();

    const colEl = (document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null)?.closest<HTMLElement>(
      "[data-column-id]",
    );
    if (!colEl) return;
    const toColumn = colEl.dataset.columnId!;
    const state = this.store.getState();
    const list = sortedColumn(state.cards, toColumn).filter((c) => c.id !== drag.cardId);
    const idx = dropIndex(colEl, e.clientY, drag.cardId);
    const before = idx > 0 ? list[idx - 1].rank : null;
    const after = idx < list.length ? list[idx].rank : null;
    const card = state.cards[drag.cardId];
    if (!card) return;
    this.store.dispatch(moveCard(card, toColumn, rankBetween(before, after)));
  };

  destroy(): void {
    this.root.removeEventListener("pointerdown", this.onDown);
  }
}
