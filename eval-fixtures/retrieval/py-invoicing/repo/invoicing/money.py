"""Decimal helpers for currency amounts.

All monetary values are ``Decimal``. Each currency has a number of minor
units (cents for USD, none for JPY); amounts are quantized to that precision
using banker's rounding unless a caller asks otherwise.
"""

from __future__ import annotations

from decimal import ROUND_HALF_EVEN, ROUND_HALF_UP, Decimal

MINOR_UNITS: dict[str, int] = {
    "USD": 2,
    "EUR": 2,
    "GBP": 2,
    "CAD": 2,
    "AUD": 2,
    "CHF": 2,
    "JPY": 0,
    "KRW": 0,
    "BHD": 3,
}

ZERO = Decimal("0")


def exponent_for(currency: str) -> Decimal:
    """Return the quantization exponent for ``currency`` (e.g. 0.01)."""
    places = MINOR_UNITS.get(currency.upper(), 2)
    return Decimal(1).scaleb(-places)


def quantize(amount: Decimal | int | str, currency: str, *, half_up: bool = False) -> Decimal:
    """Round ``amount`` to the smallest unit of ``currency``."""
    mode = ROUND_HALF_UP if half_up else ROUND_HALF_EVEN
    return Decimal(amount).quantize(exponent_for(currency), rounding=mode)


def allocate(total: Decimal, weights: list[Decimal], currency: str) -> list[Decimal]:
    """Split ``total`` across ``weights`` so the parts sum exactly to total.

    Remainders left by rounding are handed out one minor unit at a time to
    the parts with the largest fractional loss.
    """
    if not weights:
        return []
    weight_sum = sum(weights) or Decimal(1)
    raw = [total * w / weight_sum for w in weights]
    parts = [quantize(r, currency) for r in raw]
    unit = exponent_for(currency)
    diff = total - sum(parts)
    order = sorted(range(len(raw)), key=lambda i: raw[i] - parts[i], reverse=diff > 0)
    i = 0
    while diff != 0:
        step = unit if diff > 0 else -unit
        parts[order[i % len(order)]] += step
        diff -= step
        i += 1
    return parts


def format_amount(amount: Decimal, currency: str) -> str:
    """Human-readable amount, e.g. ``USD 1,234.50``."""
    places = MINOR_UNITS.get(currency.upper(), 2)
    q = quantize(amount, currency)
    return f"{currency.upper()} {q:,.{places}f}"


def to_minor(amount: Decimal, currency: str) -> int:
    """Convert to an integer count of minor units (for storage)."""
    places = MINOR_UNITS.get(currency.upper(), 2)
    return int(quantize(amount, currency).scaleb(places))


def from_minor(value: int, currency: str) -> Decimal:
    places = MINOR_UNITS.get(currency.upper(), 2)
    return Decimal(value).scaleb(-places)
