"""Compute subtotal, discount, tax and grand total for an invoice.

Each line is priced and rounded individually, then the rounded lines are
summed. Tax is computed per line on the discounted line amount and also
rounded per line before summing.
"""

from __future__ import annotations

from decimal import Decimal

from invoicing.billing import coupons, rules
from invoicing.models import Customer, Invoice, LineItem
from invoicing.money import ZERO, allocate, quantize


def line_amount(line: LineItem, currency: str) -> Decimal:
    """Quantity times unit price, rounded to the currency's precision."""
    return quantize(line.quantity * line.unit_price, currency)


def _discount_shares(inv: Invoice, amounts: list[Decimal], discount: Decimal) -> list[Decimal]:
    """Spread an invoice-level discount across discountable lines."""
    weights = [amt if line.discountable else ZERO for line, amt in zip(inv.lines, amounts)]
    if not any(weights):
        return [ZERO] * len(amounts)
    return allocate(discount, weights, inv.currency)


def recalculate(inv: Invoice, customer: Customer, *, late_fee: Decimal = ZERO) -> Invoice:
    """Recompute all monetary fields of ``inv`` in place and return it."""
    currency = inv.currency
    amounts = [line_amount(line, currency) for line in inv.lines]
    subtotal = sum(amounts, ZERO)

    discount = ZERO
    if inv.coupon_code:
        discount = coupons.discount_for(inv.coupon_code, subtotal, currency, customer)
    shares = _discount_shares(inv, amounts, discount)

    tax_total = ZERO
    for line, amount, share in zip(inv.lines, amounts, shares):
        taxable = amount - share
        rate = rules.rate_for(customer, line.tax_category, inv.issue_date)
        tax_total += quantize(taxable * rate, currency)

    inv.subtotal = subtotal
    inv.discount = discount
    inv.tax = tax_total
    inv.total = subtotal - discount + tax_total + quantize(late_fee, currency)
    return inv


def summarize(inv: Invoice) -> dict[str, str]:
    """Plain-string breakdown used by exports and the API."""
    return {
        "subtotal": str(inv.subtotal),
        "discount": str(inv.discount),
        "tax": str(inv.tax),
        "total": str(inv.total),
        "paid": str(inv.amount_paid),
        "balance": str(inv.balance),
    }
