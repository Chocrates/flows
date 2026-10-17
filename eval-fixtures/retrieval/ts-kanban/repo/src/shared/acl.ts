/**
 * Who may do what on a board. Used by the UI to hide controls and by the
 * server middleware to reject requests; the server check is authoritative.
 */
import type { BoardRecord, Id, RoleName } from "./types";

export type Capability =
  | "card.create"
  | "card.edit"
  | "card.move"
  | "card.delete"
  | "card.comment"
  | "column.create"
  | "column.rename"
  | "column.delete"
  | "board.rename"
  | "board.invite"
  | "board.changeRole"
  | "board.delete";

const GRANTS: Record<RoleName, ReadonlySet<Capability>> = {
  owner: new Set<Capability>([
    "card.create", "card.edit", "card.move", "card.delete", "card.comment",
    "column.create", "column.rename", "column.delete",
    "board.rename", "board.invite", "board.changeRole", "board.delete",
  ]),
  admin: new Set<Capability>([
    "card.create", "card.edit", "card.move", "card.delete", "card.comment",
    "column.create", "column.rename", "column.delete",
    "board.rename", "board.invite", "board.changeRole",
  ]),
  editor: new Set<Capability>([
    "card.create", "card.edit", "card.move", "card.delete", "card.comment",
    "column.create", "column.rename",
  ]),
  commenter: new Set<Capability>(["card.comment"]),
  viewer: new Set<Capability>([]),
};

const RANK: Record<RoleName, number> = { viewer: 0, commenter: 1, editor: 2, admin: 3, owner: 4 };

export function roleOf(board: BoardRecord, userId: Id): RoleName | null {
  const m = board.members.find((x) => x.userId === userId);
  return m ? m.role : null;
}

export function can(role: RoleName | null, cap: Capability): boolean {
  if (role === null) return false;
  return GRANTS[role].has(cap);
}

/**
 * An actor may only assign roles strictly below their own, and may never
 * demote someone at or above their own level.
 */
export function canAssignRole(actor: RoleName, target: RoleName, next: RoleName): boolean {
  if (!can(actor, "board.changeRole")) return false;
  if (actor === "owner") return next !== "owner" || target === "owner";
  return RANK[next] < RANK[actor] && RANK[target] < RANK[actor];
}
