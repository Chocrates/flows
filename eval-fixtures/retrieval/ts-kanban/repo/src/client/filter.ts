/**
 * Query language for the board search box:
 *
 *   free text          matches title and description
 *   label:bug          cards carrying the label named "bug"
 *   @alice             cards assigned to a member whose name starts with alice
 *   due:overdue|today|week
 *   is:archived
 */
import type { BoardRecord, CardRecord } from "../shared/types";

export interface ParsedQuery {
  text: string[];
  label: string | null;
  assignee: string | null;
  due: "overdue" | "today" | "week" | null;
  archived: boolean;
}

export function parseQuery(input: string): ParsedQuery {
  const q: ParsedQuery = { text: [], label: null, assignee: null, due: null, archived: false };
  for (const tok of input.trim().split(/\s+/).filter(Boolean)) {
    if (tok.startsWith("label:")) q.label = tok.slice(6);
    else if (tok.startsWith("@")) q.assignee = tok.slice(1).toLowerCase();
    else if (tok.startsWith("due:")) q.due = tok.slice(4) as ParsedQuery["due"];
    else if (tok === "is:archived") q.archived = true;
    else q.text.push(tok.toLowerCase());
  }
  return q;
}

function dueMatches(card: CardRecord, due: ParsedQuery["due"], now: Date): boolean {
  if (!due) return true;
  if (!card.dueAt) return false;
  const t = new Date(card.dueAt).getTime();
  const n = now.getTime();
  if (due === "overdue") return t < n;
  if (due === "today") return new Date(card.dueAt).toDateString() === now.toDateString();
  return t >= n && t < n + 7 * 86400_000;
}

export function matchCards(
  cards: CardRecord[],
  board: BoardRecord,
  query: ParsedQuery,
  now: Date = new Date(),
): CardRecord[] {
  const labelId = query.label ? board.labels.find((l) => l.name === query.label)?.id ?? "__none__" : null;
  const assigneeIds = query.assignee
    ? new Set(board.members.filter((m) => m.displayName.toLowerCase().startsWith(query.assignee!)).map((m) => m.userId))
    : null;

  return cards.filter((c) => {
    if (c.archived !== query.archived) return false;
    if (labelId && !c.labels.includes(labelId)) return false;
    if (assigneeIds && !c.assignees.some((a) => assigneeIds.has(a))) return false;
    if (!dueMatches(c, query.due, now)) return false;
    const hay = (c.title + "\n" + c.description).toLowerCase();
    return query.text.every((w) => hay.includes(w));
  });
}
