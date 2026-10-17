/**
 * Hand-rolled request body validators. Each returns either the narrowed
 * value or a map of field -> message suitable for a 400 response.
 */
import type { CardRecord, RoleName } from "../shared/types";

export type Checked<T> = { ok: true; value: T } | { ok: false; errors: Record<string, string> };

const ROLE_NAMES: RoleName[] = ["owner", "admin", "editor", "commenter", "viewer"];
const RANK_RE = /^[0-9a-z]{1,48}$/;

function isIsoDate(v: unknown): v is string {
  return typeof v === "string" && !Number.isNaN(Date.parse(v)) && /T.*Z$/.test(v);
}

export interface NewCardBody {
  id: string;
  columnId: string;
  title: string;
  description?: string;
  rank: string;
  dueAt?: string | null;
}

export function checkNewCard(body: unknown): Checked<NewCardBody> {
  const errors: Record<string, string> = {};
  const b = (body ?? {}) as Record<string, unknown>;
  if (typeof b.id !== "string") errors.id = "required";
  if (typeof b.columnId !== "string") errors.columnId = "required";
  if (typeof b.title !== "string") errors.title = "required";
  else if (b.title.length > 500) errors.title = "too long";
  if (b.description !== undefined && typeof b.description !== "string") errors.description = "must be text";
  if (typeof b.rank !== "string" || !RANK_RE.test(b.rank)) errors.rank = "invalid";
  if (b.dueAt !== undefined && b.dueAt !== null && !isIsoDate(b.dueAt)) errors.dueAt = "must be ISO-8601 UTC";
  return Object.keys(errors).length ? { ok: false, errors } : { ok: true, value: b as unknown as NewCardBody };
}

const PATCHABLE: (keyof CardRecord)[] = ["title", "description", "labels", "assignees", "dueAt", "archived"];

export function checkCardPatch(body: unknown): Checked<{ baseVersion: number; patch: Partial<CardRecord> }> {
  const b = (body ?? {}) as Record<string, unknown>;
  const errors: Record<string, string> = {};
  if (typeof b.baseVersion !== "number") errors.baseVersion = "required";
  const patch = (b.patch ?? {}) as Record<string, unknown>;
  for (const k of Object.keys(patch)) {
    if (!PATCHABLE.includes(k as keyof CardRecord)) errors[k] = "not patchable";
  }
  if ("dueAt" in patch && patch.dueAt !== null && !isIsoDate(patch.dueAt)) errors.dueAt = "must be ISO-8601 UTC";
  return Object.keys(errors).length
    ? { ok: false, errors }
    : { ok: true, value: { baseVersion: b.baseVersion as number, patch: patch as Partial<CardRecord> } };
}

export function checkMove(body: unknown): Checked<{ baseVersion: number; columnId: string; rank: string }> {
  const b = (body ?? {}) as Record<string, unknown>;
  const errors: Record<string, string> = {};
  if (typeof b.baseVersion !== "number") errors.baseVersion = "required";
  if (typeof b.columnId !== "string") errors.columnId = "required";
  if (typeof b.rank !== "string" || !RANK_RE.test(b.rank)) errors.rank = "invalid";
  return Object.keys(errors).length ? { ok: false, errors } : { ok: true, value: b as never };
}

export function checkRole(v: unknown): v is RoleName {
  return typeof v === "string" && (ROLE_NAMES as string[]).includes(v);
}
