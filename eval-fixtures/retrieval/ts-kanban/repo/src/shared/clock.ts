/**
 * Date helpers. All timestamps travel as ISO-8601 UTC strings; conversion to
 * the viewer's wall clock happens only at display time.
 */

export interface ZonedParts {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
}

/** Break a UTC instant into wall-clock parts in `timeZone`. */
export function zonedParts(iso: string, timeZone: string): ZonedParts {
  const fmt = new Intl.DateTimeFormat("en-US", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
  const parts: Record<string, string> = {};
  for (const p of fmt.formatToParts(new Date(iso))) parts[p.type] = p.value;
  return {
    year: Number(parts.year),
    month: Number(parts.month),
    day: Number(parts.day),
    hour: Number(parts.hour),
    minute: Number(parts.minute),
  };
}

/** Offset of `timeZone` from UTC at `iso`, in minutes (east positive). */
export function offsetMinutes(iso: string, timeZone: string): number {
  const p = zonedParts(iso, timeZone);
  const asUtc = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute);
  const actual = new Date(iso);
  actual.setUTCSeconds(0, 0);
  return Math.round((asUtc - actual.getTime()) / 60000);
}

/**
 * Start of the local calendar day containing `iso`, as a UTC instant.
 * NOTE: uses the offset at `iso`, which is wrong on DST transition days.
 */
export function startOfLocalDay(iso: string, timeZone: string): Date {
  const p = zonedParts(iso, timeZone);
  const midnightUtc = Date.UTC(p.year, p.month - 1, p.day);
  return new Date(midnightUtc - offsetMinutes(iso, timeZone) * 60000);
}

export function formatDue(iso: string, locale: string, timeZone: string): string {
  return new Intl.DateTimeFormat(locale, {
    timeZone,
    weekday: "short",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(iso));
}

/** "in 3 days", "2 hours ago" — coarse relative time for card badges. */
export function relativeFrom(iso: string, now: Date, locale: string): string {
  const diff = new Date(iso).getTime() - now.getTime();
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  const abs = Math.abs(diff);
  if (abs < 3600_000) return rtf.format(Math.round(diff / 60000), "minute");
  if (abs < 86400_000) return rtf.format(Math.round(diff / 3600_000), "hour");
  return rtf.format(Math.round(diff / 86400_000), "day");
}
