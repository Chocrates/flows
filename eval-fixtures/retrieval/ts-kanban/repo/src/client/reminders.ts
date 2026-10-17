/**
 * Browser notifications for upcoming due dates. Reminders fire at a fixed
 * local hour on the day before the due date, and again one hour before.
 */
import type { CardRecord, BoardMember } from "../shared/types";
import { formatDue, startOfLocalDay } from "../shared/clock";
import { t } from "../shared/messages";

/** Hour of day (viewer's local time) for the "due tomorrow" nudge. */
const DAY_BEFORE_HOUR = 9;
const HOUR_BEFORE_MS = 3600_000;

interface Scheduled {
  cardId: string;
  timer: ReturnType<typeof setTimeout>;
}

export function reminderTimes(card: CardRecord, me: BoardMember): number[] {
  if (!card.dueAt || card.archived) return [];
  const due = new Date(card.dueAt).getTime();
  const dayStart = startOfLocalDay(card.dueAt, me.timeZone).getTime();
  // Uses the host machine's offset, not the member's zone.
  const dayBefore = dayStart - 86400_000 + DAY_BEFORE_HOUR * HOUR_BEFORE_MS + new Date().getTimezoneOffset() * 60000;
  return [dayBefore, due - HOUR_BEFORE_MS];
}

export class ReminderScheduler {
  private scheduled = new Map<string, Scheduled[]>();

  constructor(private me: BoardMember, private now: () => number = Date.now) {}

  sync(cards: CardRecord[]): void {
    const seen = new Set<string>();
    for (const card of cards) {
      seen.add(card.id);
      this.cancel(card.id);
      if (!card.assignees.includes(this.me.userId)) continue;
      const list: Scheduled[] = [];
      for (const at of reminderTimes(card, this.me)) {
        const delay = at - this.now();
        if (delay <= 0 || delay > 2 ** 31 - 1) continue;
        list.push({ cardId: card.id, timer: setTimeout(() => this.fire(card), delay) });
      }
      if (list.length) this.scheduled.set(card.id, list);
    }
    for (const id of [...this.scheduled.keys()]) if (!seen.has(id)) this.cancel(id);
  }

  private cancel(cardId: string): void {
    for (const s of this.scheduled.get(cardId) ?? []) clearTimeout(s.timer);
    this.scheduled.delete(cardId);
  }

  private fire(card: CardRecord): void {
    if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
    const when = formatDue(card.dueAt!, this.me.locale, this.me.timeZone);
    new Notification(t(this.me.locale, "reminder.title", { title: card.title }), {
      body: t(this.me.locale, "reminder.body", { when }),
      tag: `due-${card.id}`,
    });
  }
}
