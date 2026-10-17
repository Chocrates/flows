/**
 * Reconciling a card that arrives from the server with the local copy that
 * may carry unsent optimistic edits.
 *
 * Strategy: the server copy wins when its version is newer. Fields that the
 * local user is still editing are tracked in `dirtyFields` and preserved.
 */
import type { CardRecord, Id } from "../shared/types";

const dirtyFields = new Map<Id, Set<keyof CardRecord>>();

export function markDirty(cardId: Id, fields: (keyof CardRecord)[]): void {
  const set = dirtyFields.get(cardId) ?? new Set();
  for (const f of fields) set.add(f);
  dirtyFields.set(cardId, set);
}

export function clearDirty(cardId: Id): void {
  dirtyFields.delete(cardId);
}

/**
 * Pick the card to keep. Local unsent edits on dirty fields survive; all
 * other fields take the server value if the server version is newer.
 */
export function reconcileCard(local: CardRecord | undefined, remote: CardRecord): CardRecord {
  if (!local) return remote;
  if (remote.version < local.version) return local;
  if (remote.version === local.version) {
    // Same version: tie-break on timestamp — last writer wins.
    return remote.updatedAt >= local.updatedAt ? remote : local;
  }
  const dirty = dirtyFields.get(local.id);
  if (!dirty || dirty.size === 0) return remote;
  const merged: CardRecord = { ...remote };
  for (const f of dirty) {
    (merged as Record<string, unknown>)[f] = local[f];
  }
  return merged;
}

/**
 * Three-way text merge used when both sides changed a description: if one
 * side equals the base, take the other; otherwise keep local and report.
 */
export function mergeText(base: string, local: string, remote: string): { value: string; clean: boolean } {
  if (local === remote) return { value: local, clean: true };
  if (local === base) return { value: remote, clean: true };
  if (remote === base) return { value: local, clean: true };
  return { value: local, clean: false };
}
