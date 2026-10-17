/**
 * Domain model shared by the browser bundle and the API server.
 *
 * Every mutable entity carries a `version` that the server bumps on each
 * accepted write; clients use it to detect concurrent edits.
 */

export type Id = string;

export type RoleName = "owner" | "admin" | "editor" | "commenter" | "viewer";

export interface BoardMember {
  userId: Id;
  displayName: string;
  role: RoleName;
  /** IANA zone, e.g. "Europe/Berlin". Used when rendering due dates. */
  timeZone: string;
  locale: string;
}

export interface LabelDef {
  id: Id;
  name: string;
  color: string;
}

export interface CardRecord {
  id: Id;
  boardId: Id;
  columnId: Id;
  title: string;
  /** Markdown source; rendered (and sanitized) on the client. */
  description: string;
  /** Lexicographic sort key, see shared/ordering.ts. */
  rank: string;
  labels: Id[];
  assignees: Id[];
  dueAt: string | null;
  archived: boolean;
  version: number;
  updatedAt: string;
  updatedBy: Id;
}

export interface ColumnRecord {
  id: Id;
  boardId: Id;
  name: string;
  rank: string;
  wipLimit: number | null;
  version: number;
}

export interface BoardRecord {
  id: Id;
  name: string;
  ownerId: Id;
  members: BoardMember[];
  labels: LabelDef[];
  version: number;
  createdAt: string;
}

/** Normalised client-side snapshot of one open board. */
export interface BoardSnapshot {
  board: BoardRecord;
  columns: Record<Id, ColumnRecord>;
  cards: Record<Id, CardRecord>;
}

/** Events pushed from the server over the realtime channel. */
export type ServerEvent =
  | { kind: "card.upserted"; card: CardRecord; originClientId: string | null }
  | { kind: "card.removed"; cardId: Id; boardId: Id; originClientId: string | null }
  | { kind: "column.upserted"; column: ColumnRecord; originClientId: string | null }
  | { kind: "column.removed"; columnId: Id; boardId: Id; originClientId: string | null }
  | { kind: "member.changed"; boardId: Id; member: BoardMember };

export interface ApiErrorBody {
  code: "validation" | "forbidden" | "not_found" | "conflict" | "internal";
  message: string;
  details?: Record<string, string>;
  /** Present on `conflict`: the server's current copy of the entity. */
  current?: unknown;
}
