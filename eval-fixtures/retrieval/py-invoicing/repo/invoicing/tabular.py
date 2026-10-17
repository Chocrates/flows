"""Spreadsheet-friendly exports (CSV) for accountants."""

from __future__ import annotations

import csv
import io
from datetime import date
from typing import Iterable

from invoicing.models import Invoice, Payment

INVOICE_COLUMNS = [
    "number", "customer_id", "currency", "issue_date", "due_date", "status",
    "subtotal", "discount", "tax", "total", "paid", "balance",
]

PAYMENT_COLUMNS = ["invoice_id", "received_on", "amount", "currency", "method", "reference"]


def _fmt(value) -> str:
    if value is None:
        return ""
    if isinstance(value, date):
        return value.isoformat()
    return str(value)


def invoices_csv(rows: Iterable[Invoice], *, delimiter: str = ",") -> str:
    out = io.StringIO()
    writer = csv.writer(out, delimiter=delimiter, lineterminator="\n")
    writer.writerow(INVOICE_COLUMNS)
    for inv in rows:
        writer.writerow([
            _fmt(inv.number), inv.customer_id, inv.currency, _fmt(inv.issue_date),
            _fmt(inv.due_date), inv.status.value, _fmt(inv.subtotal), _fmt(inv.discount),
            _fmt(inv.tax), _fmt(inv.total), _fmt(inv.amount_paid), _fmt(inv.balance),
        ])
    return out.getvalue()


def payments_csv(rows: Iterable[Payment], *, delimiter: str = ",") -> str:
    out = io.StringIO()
    writer = csv.writer(out, delimiter=delimiter, lineterminator="\n")
    writer.writerow(PAYMENT_COLUMNS)
    for p in rows:
        writer.writerow([p.invoice_id, _fmt(p.received_on), _fmt(p.amount), p.currency,
                         p.method, _fmt(p.reference)])
    return out.getvalue()


def in_period(rows: Iterable[Invoice], start: date, end: date) -> list[Invoice]:
    """Bills issued within [start, end]."""
    return [r for r in rows if r.issue_date and start <= r.issue_date <= end]
