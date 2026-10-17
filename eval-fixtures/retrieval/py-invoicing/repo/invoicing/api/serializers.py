"""JSON shapes for API responses and request parsing."""

from __future__ import annotations

from datetime import date
from decimal import Decimal, InvalidOperation
from typing import Any

from invoicing.models import Customer, Invoice, LineItem, Payment


class ValidationError(ValueError):
    pass


def _money(value: Decimal) -> str:
    return format(value, "f")


def invoice_to_json(inv: Invoice) -> dict[str, Any]:
    return {
        "id": inv.id,
        "number": inv.number,
        "customer_id": inv.customer_id,
        "currency": inv.currency,
        "status": inv.status.value,
        "issue_date": inv.issue_date.isoformat() if inv.issue_date else None,
        "due_date": inv.due_date.isoformat() if inv.due_date else None,
        "coupon_code": inv.coupon_code,
        "lines": [
            {"description": l.description, "quantity": str(l.quantity),
             "unit_price": _money(l.unit_price), "tax_category": l.tax_category}
            for l in inv.lines
        ],
        "subtotal": _money(inv.subtotal),
        "discount": _money(inv.discount),
        "tax": _money(inv.tax),
        "total": _money(inv.total),
        "amount_paid": _money(inv.amount_paid),
        "balance": _money(inv.balance),
        "reminders_sent": inv.reminders_sent,
        "notes": inv.notes,
    }


def customer_to_json(c: Customer) -> dict[str, Any]:
    return {"id": c.id, "name": c.name, "email": c.email, "country": c.country,
            "region": c.region, "vat_id": c.vat_id, "currency": c.currency}


def payment_to_json(p: Payment) -> dict[str, Any]:
    return {"id": p.id, "invoice_id": p.invoice_id, "amount": _money(p.amount),
            "currency": p.currency, "received_on": p.received_on.isoformat(), "method": p.method}


def _decimal(value: Any, field: str) -> Decimal:
    try:
        return Decimal(str(value))
    except (InvalidOperation, TypeError):
        raise ValidationError(f"{field} must be a number") from None


def parse_lines(payload: list[dict[str, Any]]) -> list[LineItem]:
    if not isinstance(payload, list) or not payload:
        raise ValidationError("lines must be a non-empty list")
    out = []
    for i, item in enumerate(payload):
        if "description" not in item:
            raise ValidationError(f"lines[{i}].description is required")
        out.append(LineItem(
            description=str(item["description"]),
            quantity=_decimal(item.get("quantity", 1), f"lines[{i}].quantity"),
            unit_price=_decimal(item.get("unit_price"), f"lines[{i}].unit_price"),
            tax_category=item.get("tax_category", "standard"),
            discountable=bool(item.get("discountable", True)),
        ))
    return out


def parse_date(value: Any, field: str) -> date:
    try:
        return date.fromisoformat(str(value))
    except ValueError:
        raise ValidationError(f"{field} must be YYYY-MM-DD") from None
