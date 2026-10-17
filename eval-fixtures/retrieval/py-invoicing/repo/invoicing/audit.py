"""Append-only trail of who changed what.

Every entry is one JSON object per line in the file named by
``audit_log_path``. Entries are never rewritten; rotation is handled by the
host's logrotate.
"""

from __future__ import annotations

import json
import threading
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from invoicing.settings import get_config

_lock = threading.Lock()
_REDACT = {"password", "api_key", "card_number", "iban"}


def _scrub(details: dict[str, Any]) -> dict[str, Any]:
    return {k: ("***" if k in _REDACT else v) for k, v in details.items()}


def record(actor: str, action: str, entity: str, entity_id: int | str | None,
           details: dict[str, Any] | None = None, *, path: str | None = None) -> dict[str, Any]:
    """Write one entry and return it."""
    entry = {
        "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "actor": actor,
        "action": action,
        "entity": entity,
        "entity_id": entity_id,
        "details": _scrub(details or {}),
    }
    target = Path(path or get_config().audit_log_path)
    line = json.dumps(entry, sort_keys=True, default=str)
    with _lock:
        with target.open("a", encoding="utf-8") as fh:
            fh.write(line + "\n")
    return entry


def history(entity: str, entity_id: int | str, *, path: str | None = None) -> list[dict[str, Any]]:
    """All entries for one entity, oldest first."""
    target = Path(path or get_config().audit_log_path)
    if not target.exists():
        return []
    out = []
    for raw in target.read_text(encoding="utf-8").splitlines():
        entry = json.loads(raw)
        if entry["entity"] == entity and str(entry["entity_id"]) == str(entity_id):
            out.append(entry)
    return out
