/**
 * Fractional rank keys.
 *
 * Cards and columns are sorted by a base-36 string key instead of an integer
 * index so that moving one item only rewrites that item's key. A new key is
 * always strictly between its neighbours' keys.
 */

const DIGITS = "0123456789abcdefghijklmnopqrstuvwxyz";
const BASE = DIGITS.length;
const MIN_CHAR = DIGITS[0];
const MAX_CHAR = DIGITS[BASE - 1];

/** Longest key we accept before asking the server to renumber a column. */
export const MAX_RANK_LENGTH = 24;

function digitAt(key: string, i: number, pad: string): number {
  const ch = i < key.length ? key[i] : pad;
  return DIGITS.indexOf(ch);
}

/**
 * Return a key strictly between `before` and `after`.
 * `null` means "open end" (start or end of the list).
 */
export function rankBetween(before: string | null, after: string | null): string {
  const lo = before ?? "";
  const hi = after ?? "";
  if (before !== null && after !== null && lo >= hi) {
    throw new Error(`rankBetween: bounds out of order (${lo} >= ${hi})`);
  }

  let out = "";
  for (let i = 0; ; i++) {
    const a = digitAt(lo, i, MIN_CHAR);
    const b = after === null ? BASE : digitAt(hi, i, MAX_CHAR) + (i < hi.length ? 0 : 1);
    if (b - a > 1) {
      // Room for a midpoint digit at this position.
      return out + DIGITS[Math.floor((a + b) / 2)];
    }
    // No room: copy the lower digit and descend one position.
    out += DIGITS[a];
    if (out.length > MAX_RANK_LENGTH * 2) {
      throw new Error("rankBetween: key space exhausted");
    }
  }
}

/** True when keys in a column have grown long enough to warrant a rebalance. */
export function needsRebalance(keys: string[]): boolean {
  return keys.some((k) => k.length > MAX_RANK_LENGTH);
}

/** Evenly spaced fresh keys for `count` items, used by the server rebalance job. */
export function spreadRanks(count: number): string[] {
  const out: string[] = [];
  const width = Math.max(2, Math.ceil(Math.log(count + 1) / Math.log(BASE)) + 1);
  const span = Math.pow(BASE, width);
  const step = Math.floor(span / (count + 1));
  for (let i = 1; i <= count; i++) {
    out.push((i * step).toString(BASE).padStart(width, MIN_CHAR));
  }
  return out;
}

export function compareRank(a: { rank: string; id: string }, b: { rank: string; id: string }): number {
  if (a.rank < b.rank) return -1;
  if (a.rank > b.rank) return 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}
