"""Date helpers: today(), business days and due-date arithmetic."""

from __future__ import annotations

from datetime import date, datetime, timedelta, timezone

# Fixed-date public holidays observed by the finance team.
HOLIDAYS: set[tuple[int, int]] = {(1, 1), (12, 25), (12, 26)}

_frozen: date | None = None


def today() -> date:
    """Current UTC date; can be frozen in tests via ``freeze``."""
    if _frozen is not None:
        return _frozen
    return datetime.now(timezone.utc).date()


def freeze(on: date | None) -> None:
    global _frozen
    _frozen = on


def is_business_day(d: date) -> bool:
    return d.weekday() < 5 and (d.month, d.day) not in HOLIDAYS


def add_days(start: date, days: int) -> date:
    """Calendar-day offset; if the result lands on a non-working day it is
    pushed forward to the next working day."""
    result = start + timedelta(days=days)
    while not is_business_day(result):
        result += timedelta(days=1)
    return result


def add_business_days(start: date, days: int) -> date:
    d = start
    remaining = days
    while remaining > 0:
        d += timedelta(days=1)
        if is_business_day(d):
            remaining -= 1
    return d


def due_date_for(issue: date, terms_days: int) -> date:
    """Due date for net-N payment terms."""
    return add_days(issue, terms_days)


def days_between(a: date, b: date) -> int:
    return (b - a).days
