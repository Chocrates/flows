"""Lifecycle operations on bills: create draft, issue, void."""

from __future__ import annotations

from invoicing import audit, clock, emails, pdf
from invoicing.billing import coupons, numbering, recalculate
from invoicing.mailer import Mailer, Outgoing
from invoicing.models import Invoice, InvoiceStatus, LineItem
from invoicing.settings import get_config
from invoicing.storage.db import Database
from invoicing.storage.repository import CustomerRepository, InvoiceRepository


class LifecycleError(RuntimeError):
    pass


class BillingService:
    def __init__(self, db: Database, mailer: Mailer) -> None:
        self.db = db
        self.invoices = InvoiceRepository(db)
        self.customers = CustomerRepository(db)
        self.mailer = mailer

    def create_draft(self, customer_id: int, lines: list[LineItem], *,
                     coupon_code: str | None = None, actor: str) -> Invoice:
        customer = self.customers.get(customer_id)
        if customer is None:
            raise LifecycleError(f"no customer {customer_id}")
        inv = Invoice(id=None, number=None, customer_id=customer_id, currency=customer.currency,
                      issue_date=None, due_date=None, lines=lines, coupon_code=coupon_code)
        recalculate(inv, customer)
        self.invoices.save(inv)
        audit.record(actor, "invoice.created", "invoice", inv.id, {"total": str(inv.total)})
        return inv

    def issue(self, invoice_id: int, *, actor: str, send: bool = True) -> Invoice:
        inv = self.invoices.get(invoice_id)
        if inv is None or inv.status != InvoiceStatus.DRAFT:
            raise LifecycleError("only drafts can be issued")
        customer = self.customers.get(inv.customer_id)
        today = clock.today()
        inv.issue_date = today
        inv.due_date = clock.due_date_for(today, get_config().payment_terms_days)
        recalculate(inv, customer)
        inv.number = numbering.next_number(self.db, today)
        inv.status = InvoiceStatus.ISSUED
        if inv.coupon_code:
            coupons.redeem(inv.coupon_code)
        self.invoices.save(inv)
        audit.record(actor, "invoice.issued", "invoice", inv.id, {"number": inv.number})
        if send:
            subject, body = emails.render("issued", inv, customer)
            attachment = (f"{inv.number}.pdf", pdf.render_pdf(inv, customer), "application/pdf")
            self.mailer.send(Outgoing(customer.email, subject, body, [attachment]))
        return inv
