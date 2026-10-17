"""Thin SQLite wrapper and schema bootstrap."""

from __future__ import annotations

import sqlite3
import threading
from contextlib import contextmanager
from typing import Any, Iterator

SCHEMA = """
CREATE TABLE IF NOT EXISTS customers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    country TEXT NOT NULL,
    region TEXT,
    vat_id TEXT,
    tax_exempt INTEGER NOT NULL DEFAULT 0,
    currency TEXT NOT NULL DEFAULT 'USD',
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS invoices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    number TEXT UNIQUE,
    customer_id INTEGER NOT NULL REFERENCES customers(id),
    currency TEXT NOT NULL,
    issue_date TEXT,
    due_date TEXT,
    status TEXT NOT NULL,
    coupon_code TEXT,
    subtotal_minor INTEGER NOT NULL DEFAULT 0,
    discount_minor INTEGER NOT NULL DEFAULT 0,
    tax_minor INTEGER NOT NULL DEFAULT 0,
    total_minor INTEGER NOT NULL DEFAULT 0,
    paid_minor INTEGER NOT NULL DEFAULT 0,
    reminders_sent INTEGER NOT NULL DEFAULT 0,
    last_reminder_on TEXT,
    notes TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS line_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    invoice_id INTEGER NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    description TEXT NOT NULL,
    quantity TEXT NOT NULL,
    unit_price TEXT NOT NULL,
    tax_category TEXT NOT NULL,
    discountable INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS payments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    invoice_id INTEGER NOT NULL REFERENCES invoices(id),
    amount_minor INTEGER NOT NULL,
    currency TEXT NOT NULL,
    received_on TEXT NOT NULL,
    method TEXT NOT NULL,
    reference TEXT
);
CREATE TABLE IF NOT EXISTS sequences (
    name TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS api_keys (
    key_hash TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    scopes TEXT NOT NULL,
    revoked_at TEXT
);
"""


class Database:
    """A single connection guarded by a lock; good enough for one process."""

    def __init__(self, path: str) -> None:
        self.path = path
        self._conn = sqlite3.connect(path, check_same_thread=False, isolation_level=None)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA foreign_keys = ON")
        self._lock = threading.RLock()

    def init_schema(self) -> None:
        with self._lock:
            self._conn.executescript(SCHEMA)

    def execute(self, sql: str, params: tuple[Any, ...] = ()) -> sqlite3.Cursor:
        with self._lock:
            return self._conn.execute(sql, params)

    def query_one(self, sql: str, params: tuple[Any, ...] = ()) -> sqlite3.Row | None:
        with self._lock:
            return self._conn.execute(sql, params).fetchone()

    def query_all(self, sql: str, params: tuple[Any, ...] = ()) -> list[sqlite3.Row]:
        with self._lock:
            return self._conn.execute(sql, params).fetchall()

    @contextmanager
    def transaction(self) -> Iterator["Database"]:
        """BEGIN/COMMIT block; rolls back on any exception."""
        with self._lock:
            self._conn.execute("BEGIN")
            try:
                yield self
            except Exception:
                self._conn.execute("ROLLBACK")
                raise
            else:
                self._conn.execute("COMMIT")

    def close(self) -> None:
        self._conn.close()


def open_database(path: str) -> Database:
    db = Database(path)
    db.init_schema()
    return db
