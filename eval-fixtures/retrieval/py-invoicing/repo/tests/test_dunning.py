from datetime import date
from decimal import Decimal

from invoicing import dunning
from invoicing.models import Invoice, InvoiceStatus


def _open_invoice(due, sent=0):
    return Invoice(id=7, number="INV-2026-000007", customer_id=1, currency="USD",
                   issue_date=date(2026, 1, 1), due_date=due, status=InvoiceStatus.ISSUED,
                   total=Decimal("50.00"), reminders_sent=sent)


def test_nothing_before_due_date():
    assert dunning.stage_for(_open_invoice(date(2026, 2, 1)), date(2026, 2, 1)) is None


def test_first_stage_day_after_due():
    stage = dunning.stage_for(_open_invoice(date(2026, 2, 1)), date(2026, 2, 2))
    assert stage.template == "gentle"


def test_stages_are_not_skipped():
    stage = dunning.stage_for(_open_invoice(date(2026, 1, 1)), date(2026, 3, 15), sent=1)
    assert stage.template == "firm"


def test_paid_bills_get_nothing():
    inv = _open_invoice(date(2026, 1, 1))
    inv.status = InvoiceStatus.PAID
    assert dunning.stage_for(inv, date(2026, 3, 1)) is None
