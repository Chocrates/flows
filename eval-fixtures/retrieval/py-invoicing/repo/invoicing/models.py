"""Domain objects."""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import date, datetime
from decimal import Decimal
from enum import Enum


class InvoiceStatus(str, Enum):
    DRAFT = "draft"
    ISSUED = "issued"
    PARTIALLY_PAID = "partially_paid"
    PAID = "paid"
    VOID = "void"


@dataclass
class Customer:
    id: int | None
    name: str
    email: str
    country: str
    region: str | None = None
    vat_id: str | None = None
    tax_exempt: bool = False
    currency: str = "USD"
    created_at: datetime = field(default_factory=datetime.utcnow)


@dataclass
class LineItem:
    description: str
    quantity: Decimal
    unit_price: Decimal
    tax_category: str = "standard"
    discountable: bool = True


@dataclass
class Invoice:
    id: int | None
    number: str | None
    customer_id: int
    currency: str
    issue_date: date | None
    due_date: date | None
    lines: list[LineItem] = field(default_factory=list)
    coupon_code: str | None = None
    status: InvoiceStatus = InvoiceStatus.DRAFT
    subtotal: Decimal = Decimal("0")
    discount: Decimal = Decimal("0")
    tax: Decimal = Decimal("0")
    total: Decimal = Decimal("0")
    amount_paid: Decimal = Decimal("0")
    reminders_sent: int = 0
    last_reminder_on: date | None = None
    notes: str = ""

    @property
    def balance(self) -> Decimal:
        return self.total - self.amount_paid

    @property
    def is_open(self) -> bool:
        return self.status in (InvoiceStatus.ISSUED, InvoiceStatus.PARTIALLY_PAID)


@dataclass
class Payment:
    id: int | None
    invoice_id: int
    amount: Decimal
    currency: str
    received_on: date
    method: str = "bank_transfer"
    reference: str | None = None
