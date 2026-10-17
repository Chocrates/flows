"""Request credential checks.

Clients send ``X-Api-Key``. Keys are stored hashed (SHA-256) alongside a
label, a comma-separated list of scopes and an optional revocation time.
"""

from __future__ import annotations

import hashlib
import hmac
import secrets
from dataclasses import dataclass
from functools import wraps
from typing import Callable

from flask import g, jsonify, request

from invoicing.storage.db import Database

HEADER = "X-Api-Key"


@dataclass
class Principal:
    label: str
    scopes: frozenset[str]

    def can(self, scope: str) -> bool:
        return "admin" in self.scopes or scope in self.scopes


def hash_key(raw: str) -> str:
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()


def issue_key(db: Database, label: str, scopes: list[str]) -> str:
    """Create a key and return the raw value (shown once)."""
    raw = "inv_" + secrets.token_urlsafe(32)
    db.execute("INSERT INTO api_keys(key_hash, label, scopes) VALUES (?, ?, ?)",
               (hash_key(raw), label, ",".join(scopes)))
    return raw


def resolve(db: Database, raw: str | None) -> Principal | None:
    if not raw:
        return None
    digest = hash_key(raw)
    row = db.query_one("SELECT key_hash, label, scopes, revoked_at FROM api_keys WHERE key_hash = ?",
                       (digest,))
    if row is None or not hmac.compare_digest(row["key_hash"], digest):
        return None
    return Principal(row["label"], frozenset(s for s in row["scopes"].split(",") if s))


def revoke(db: Database, label: str, when: str) -> None:
    db.execute("UPDATE api_keys SET revoked_at = ? WHERE label = ?", (when, label))


def require(scope: str, db_getter: Callable[[], Database]) -> Callable:
    """Decorator: reject the request unless the caller holds ``scope``."""
    def decorator(view):
        @wraps(view)
        def wrapper(*args, **kwargs):
            principal = resolve(db_getter(), request.headers.get(HEADER))
            if principal is None:
                return jsonify(error="unauthorized"), 401
            if not principal.can(scope):
                return jsonify(error="forbidden"), 403
            g.principal = principal
            return view(*args, **kwargs)
        return wrapper
    return decorator
