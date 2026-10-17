"""Escalating reminders for unpaid bills.

Stages are keyed by how many days a bill is past its due date. Each stage is
sent at most once; the stage index is tracked via ``reminders_sent``.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date

from invoicing import audit, clock, emails
from invoicing.mailer import Mailer, Outgoing
from invoicing.models import Customer, Invoice
from invoicing.storage.repository import CustomerRepository, InvoiceRepository


@dataclass(frozen=True)
class Stage:
    days_overdue: int
    template: str


STAGES: tuple[Stage, ...] = (
    Stage(1, "gentle"),
    Stage(14, "firm"),
    Stage(30, "final"),
    Stage(60, "collections"),
)


def days_overdue(inv: Invoice, on: date) -> int:
    if inv.due_date is None:
        return 0
    return max(0, clock.days_between(inv.due_date, on))


def stage_for(inv: Invoice, on: date) -> Stage | None:
    """The next stage due for ``inv`` on ``on``, or None if nothing to send."""
    late = days_overdue(inv, on)
    if late <= 0 or not inv.is_open:
        return None
    eligible = [s for s in STAGES if late >= s.days_overdue]
    if len(eligible) <= inv.reminders_sent:
        return None
    return STAGES[inv.reminders_sent]


def build_reminder(inv: Invoice, customer: Customer, stage: Stage, on: date) -> Outgoing:
    subject, body = emails.render(stage.template, inv, customer, days_overdue(inv, on))
    return Outgoing(to=customer.email, subject=subject, body=body)


def run_reminders(invoices: InvoiceRepository, customers: CustomerRepository,
                  mailer: Mailer, on: date | None = None) -> int:
    """Send whatever reminders are due today. Returns the number sent."""
    on = on or clock.today()
    sent = 0
    for inv in invoices.list_open():
        if inv.last_reminder_on == on:
            continue
        stage = stage_for(inv, on)
        if stage is None:
            continue
        customer = customers.get(inv.customer_id)
        if customer is None:
            continue
        if not mailer.send(build_reminder(inv, customer, stage, on)):
            continue
        inv.reminders_sent += 1
        inv.last_reminder_on = on
        invoices.save(inv)
        audit.record("scheduler", "reminder.sent", "invoice", inv.id, {"stage": stage.template})
        sent += 1
    return sent
