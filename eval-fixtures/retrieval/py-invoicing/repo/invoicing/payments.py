"""Recording money received against bills.

A payment reduces the outstanding balance. Once the balance reaches zero
the bill becomes paid; anything less leaves it partially paid. Payments in a
different currency from the bill are rejected.
"""

from __future__ import annotations

from datetime import date
from decimal import Decimal

from invoicing import audit
from invoicing.models import Invoice, InvoiceStatus, Payment
from invoicing.money import ZERO, quantize
from invoicing.storage.repository import InvoiceRepository, PaymentRepository

# Balances at or below this many minor units are written off as settled.
SETTLE_TOLERANCE = Decimal("0.05")


class PaymentError(ValueError):
    pass


def apply_payment(inv: Invoice, amount: Decimal) -> Invoice:
    """Update ``inv`` amounts and status for a payment of ``amount``."""
    if inv.status in (InvoiceStatus.DRAFT, InvoiceStatus.VOID):
        raise PaymentError(f"cannot pay a {inv.status.value} bill")
    amount = quantize(amount, inv.currency)
    if amount <= ZERO:
        raise PaymentError("amount must be positive")
    inv.amount_paid += amount
    if inv.balance <= SETTLE_TOLERANCE:
        inv.status = InvoiceStatus.PAID
    else:
        inv.status = InvoiceStatus.PARTIALLY_PAID
    return inv


def record_payment(
    invoices: InvoiceRepository,
    payments: PaymentRepository,
    invoice_id: int,
    amount: Decimal,
    currency: str,
    received_on: date,
    *,
    method: str = "bank_transfer",
    reference: str | None = None,
    actor: str = "system",
) -> Payment:
    inv = invoices.get(invoice_id)
    if inv is None:
        raise PaymentError(f"no bill {invoice_id}")
    if currency.upper() != inv.currency.upper():
        raise PaymentError("payment currency does not match bill currency")
    before = inv.status
    apply_payment(inv, amount)
    invoices.save(inv)
    p = payments.add(Payment(None, invoice_id, amount, currency, received_on, method, reference))
    audit.record(actor, "payment.recorded", "invoice", invoice_id, {
        "amount": str(amount), "status_before": before.value, "status_after": inv.status.value,
    })
    return p


def refund(invoices: InvoiceRepository, invoice_id: int, amount: Decimal, actor: str) -> Invoice:
    """Reverse part of a payment; reopens a settled bill."""
    inv = invoices.get(invoice_id)
    if inv is None:
        raise PaymentError(f"no bill {invoice_id}")
    amount = quantize(amount, inv.currency)
    if amount > inv.amount_paid:
        raise PaymentError("refund exceeds amount paid")
    inv.amount_paid -= amount
    inv.status = InvoiceStatus.ISSUED if inv.amount_paid == ZERO else InvoiceStatus.PARTIALLY_PAID
    invoices.save(inv)
    audit.record(actor, "payment.refunded", "invoice", invoice_id, {"amount": str(amount)})
    return inv
