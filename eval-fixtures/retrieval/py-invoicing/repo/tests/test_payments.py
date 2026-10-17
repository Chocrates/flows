from datetime import date
from decimal import Decimal

import pytest

from invoicing.models import Invoice, InvoiceStatus
from invoicing.payments import PaymentError, apply_payment


def _issued(total="100.00"):
    return Invoice(id=1, number="INV-2026-000001", customer_id=1, currency="USD",
                   issue_date=date(2026, 1, 1), due_date=date(2026, 1, 31),
                   status=InvoiceStatus.ISSUED, total=Decimal(total))


def test_full_payment_settles():
    inv = apply_payment(_issued(), Decimal("100.00"))
    assert inv.status == InvoiceStatus.PAID
    assert inv.balance == Decimal("0")


def test_partial_payment():
    inv = apply_payment(_issued(), Decimal("40"))
    assert inv.status == InvoiceStatus.PARTIALLY_PAID
    assert inv.balance == Decimal("60.00")


def test_draft_cannot_be_paid():
    inv = _issued()
    inv.status = InvoiceStatus.DRAFT
    with pytest.raises(PaymentError):
        apply_payment(inv, Decimal("1"))
