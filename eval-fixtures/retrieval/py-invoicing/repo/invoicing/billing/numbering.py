"""Sequential document numbers like ``INV-2026-000123``.

The counter is stored per year in the ``sequences`` table. Numbers must be
gap-free for accounting, so a number is only drawn when a draft is issued.
"""

from __future__ import annotations

from datetime import date

from invoicing.storage.db import Database

PREFIX = "INV"
WIDTH = 6


def format_number(year: int, seq: int, prefix: str = PREFIX) -> str:
    return f"{prefix}-{year}-{seq:0{WIDTH}d}"


def next_number(db: Database, on: date | None = None) -> str:
    """Reserve and return the next number for the year of ``on``."""
    year = (on or date.today()).year
    row = db.query_one("SELECT value FROM sequences WHERE name = ?", (f"invoice:{year}",))
    current = row["value"] if row else 0
    nxt = current + 1
    if row:
        db.execute("UPDATE sequences SET value = ? WHERE name = ?", (nxt, f"invoice:{year}"))
    else:
        db.execute("INSERT INTO sequences(name, value) VALUES (?, ?)", (f"invoice:{year}", nxt))
    return format_number(year, nxt)


def parse_number(number: str) -> tuple[str, int, int]:
    prefix, year, seq = number.split("-")
    return prefix, int(year), int(seq)
