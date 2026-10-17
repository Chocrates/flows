from datetime import date
from decimal import Decimal

from invoicing.billing import coupons
from invoicing.billing.totals import recalculate
from invoicing.models import Customer, Invoice, LineItem


def _customer(**kw):
    base = dict(id=1, name="Acme", email="ap@acme.test", country="IE", currency="EUR")
    base.update(kw)
    return Customer(**base)


def _invoice(lines, coupon=None):
    return Invoice(id=None, number=None, customer_id=1, currency="EUR",
                   issue_date=date(2026, 3, 1), due_date=None, lines=lines, coupon_code=coupon)


def test_single_line_standard_rate():
    inv = recalculate(_invoice([LineItem("Hosting", Decimal("1"), Decimal("100.00"))]), _customer())
    assert inv.subtotal == Decimal("100.00")
    assert inv.tax == Decimal("23.00")
    assert inv.total == Decimal("123.00")


def test_reverse_charge_has_no_tax():
    cust = _customer(country="DE", vat_id="DE123456789")
    inv = recalculate(_invoice([LineItem("Hosting", Decimal("1"), Decimal("100.00"))]), cust)
    assert inv.tax == Decimal("0")


def test_percentage_promo_is_spread_over_lines():
    coupons.register(coupons.Coupon("SPRING10", percent_off=Decimal("10")))
    lines = [LineItem("A", Decimal("3"), Decimal("0.35")), LineItem("B", Decimal("1"), Decimal("9.99"))]
    inv = recalculate(_invoice(lines, coupon="SPRING10"), _customer())
    assert inv.discount == Decimal("1.10")
    assert inv.total == inv.subtotal - inv.discount + inv.tax
