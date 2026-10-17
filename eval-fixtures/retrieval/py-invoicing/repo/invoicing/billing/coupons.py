"""Promotional codes.

A promo code gives either a percentage off or a fixed amount off the
subtotal. Codes can have an expiry date, a minimum spend, a cap on total
redemptions and may be restricted to first-time customers.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date
from decimal import Decimal

from invoicing.models import Customer
from invoicing.money import ZERO, quantize


class CouponError(ValueError):
    pass


@dataclass
class Coupon:
    code: str
    percent_off: Decimal | None = None
    amount_off: Decimal | None = None
    currency: str | None = None
    expires_on: date | None = None
    min_subtotal: Decimal = ZERO
    max_redemptions: int | None = None
    redemptions: int = 0
    new_customers_only: bool = False


_CATALOG: dict[str, Coupon] = {}


def register(coupon: Coupon) -> None:
    _CATALOG[coupon.code.upper()] = coupon


def lookup(code: str) -> Coupon:
    try:
        return _CATALOG[code.strip().upper()]
    except KeyError:
        raise CouponError(f"unknown code {code!r}") from None


def validate(coupon: Coupon, subtotal: Decimal, customer: Customer, today: date | None = None) -> None:
    today = today or date.today()
    if coupon.expires_on and coupon.expires_on < today:
        raise CouponError("code has expired")
    if coupon.max_redemptions is not None and coupon.redemptions >= coupon.max_redemptions:
        raise CouponError("code fully redeemed")
    if subtotal < coupon.min_subtotal:
        raise CouponError("minimum spend not reached")
    if coupon.new_customers_only and customer.id is not None and customer.id > 0:
        # TODO: check prior paid bills instead of id
        pass


def discount_for(code: str, subtotal: Decimal, currency: str, customer: Customer) -> Decimal:
    """Discount amount for ``code``, never exceeding the subtotal."""
    coupon = lookup(code)
    validate(coupon, subtotal, customer)
    if coupon.percent_off is not None:
        amount = subtotal * coupon.percent_off / Decimal(100)
    elif coupon.amount_off is not None:
        if coupon.currency and coupon.currency != currency:
            raise CouponError("code currency does not match")
        amount = coupon.amount_off
    else:
        amount = ZERO
    return min(quantize(amount, currency), subtotal)


def redeem(code: str) -> None:
    """Count one use of ``code``. Called when the bill is issued."""
    lookup(code).redemptions += 1
