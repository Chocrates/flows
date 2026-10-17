"""Configuration loading.

Values come from a YAML file (path in ``INVOICING_CONFIG``) and are then
overlaid by environment variables prefixed with ``INVOICING_``. Nested keys
use a double underscore, e.g. ``INVOICING_SMTP__HOST``.
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

ENV_PREFIX = "INVOICING_"
DEFAULT_PATH = "config.yml"


@dataclass
class SmtpConfig:
    host: str = "localhost"
    port: int = 25
    username: str | None = None
    password: str | None = None
    use_tls: bool = False
    sender: str = "billing@example.com"


@dataclass
class AppConfig:
    database_path: str = "invoicing.db"
    base_currency: str = "USD"
    payment_terms_days: int = 30
    reminder_hour_utc: int = 9
    audit_log_path: str = "audit.log"
    company_name: str = "Example Ltd"
    company_address: str = ""
    bank_details: str = ""
    smtp: SmtpConfig = field(default_factory=SmtpConfig)
    raw: dict[str, Any] = field(default_factory=dict)


def _coerce(value: str, current: Any) -> Any:
    """Convert an environment string into the type of the existing value."""
    if isinstance(current, bool):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    if isinstance(current, int):
        return int(value)
    return value


def _apply_env(data: dict[str, Any], environ: dict[str, str]) -> dict[str, Any]:
    for key, value in environ.items():
        if not key.startswith(ENV_PREFIX):
            continue
        path = key[len(ENV_PREFIX):].lower().split("__")
        node = data
        for part in path[:-1]:
            node = node.setdefault(part, {})
        node[path[-1]] = value
    return data


def load_config(path: str | None = None, environ: dict[str, str] | None = None) -> AppConfig:
    """Load configuration from disk and the environment."""
    environ = dict(os.environ if environ is None else environ)
    cfg_path = Path(path or environ.get("INVOICING_CONFIG", DEFAULT_PATH))
    data: dict[str, Any] = {}
    if cfg_path.exists():
        data = yaml.safe_load(cfg_path.read_text()) or {}
    environ.pop("INVOICING_CONFIG", None)
    data = _apply_env(data, environ)

    smtp_defaults = SmtpConfig()
    smtp_raw = data.get("smtp", {}) or {}
    smtp = SmtpConfig(**{
        k: _coerce(v, getattr(smtp_defaults, k)) if isinstance(v, str) else v
        for k, v in smtp_raw.items()
        if hasattr(smtp_defaults, k)
    })

    defaults = AppConfig()
    top = {
        k: _coerce(v, getattr(defaults, k)) if isinstance(v, str) else v
        for k, v in data.items()
        if k not in {"smtp", "raw"} and hasattr(defaults, k)
    }
    return AppConfig(smtp=smtp, raw=data, **top)


_cached: AppConfig | None = None


def get_config() -> AppConfig:
    """Return the process-wide configuration, loading it on first use."""
    global _cached
    if _cached is None:
        _cached = load_config()
    return _cached
