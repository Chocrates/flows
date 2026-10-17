"""Wording of customer-facing messages."""

from __future__ import annotations

from string import Template

from invoicing.models import Customer, Invoice
from invoicing.money import format_amount
from invoicing.settings import get_config

SUBJECTS = {
    "issued": "Your bill $number from $company",
    "gentle": "Friendly reminder: $number is now due",
    "firm": "Payment overdue: $number",
    "final": "Final notice: $number is $days days overdue",
    "collections": "Account referred: $number",
    "receipt": "Payment received for $number",
}

BODIES = {
    "issued": (
        "Hello,\n\nPlease find attached bill $number for $balance, due on $due.\n\n"
        "Thank you,\n$company"
    ),
    "gentle": (
        "Hello,\n\nThis is a quick reminder that $number ($balance) was due on $due.\n"
        "If you have already paid, please ignore this message.\n\n$company"
    ),
    "firm": (
        "Hello,\n\nOur records show $number ($balance) is $days days overdue.\n"
        "Please arrange payment at your earliest convenience.\n\n$company"
    ),
    "final": (
        "Hello,\n\n$number ($balance) is now $days days overdue. If payment is not\n"
        "received within 14 days the account will be referred for collection.\n\n$company"
    ),
    "collections": (
        "Hello,\n\n$number ($balance) has been referred to our collections partner.\n\n$company"
    ),
    "receipt": "Hello,\n\nWe received your payment towards $number. Remaining: $balance.\n\n$company",
}


def render(kind: str, inv: Invoice, customer: Customer, days: int = 0) -> tuple[str, str]:
    """Return (subject, body) for message ``kind``."""
    cfg = get_config()
    values = {
        "number": inv.number or "(draft)",
        "company": cfg.company_name,
        "balance": format_amount(inv.balance, inv.currency),
        "due": inv.due_date.isoformat() if inv.due_date else "on receipt",
        "days": str(days),
    }
    subject = Template(SUBJECTS[kind]).safe_substitute(values)
    body = Template(BODIES[kind]).safe_substitute(values)
    return subject, body
