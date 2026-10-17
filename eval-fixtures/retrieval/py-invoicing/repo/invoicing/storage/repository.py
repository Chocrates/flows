"""Load and save domain objects."""

from __future__ import annotations

from datetime import date, datetime
from decimal import Decimal

from invoicing.models import Customer, Invoice, InvoiceStatus, LineItem, Payment
from invoicing.money import from_minor, to_minor
from invoicing.storage.db import Database


def _d(value: str | None) -> date | None:
    return date.fromisoformat(value) if value else None


class CustomerRepository:
    def __init__(self, db: Database) -> None:
        self.db = db

    def get(self, customer_id: int) -> Customer | None:
        row = self.db.query_one("SELECT * FROM customers WHERE id = ?", (customer_id,))
        if row is None:
            return None
        return Customer(
            id=row["id"], name=row["name"], email=row["email"], country=row["country"],
            region=row["region"], vat_id=row["vat_id"], tax_exempt=bool(row["tax_exempt"]),
            currency=row["currency"], created_at=datetime.fromisoformat(row["created_at"]),
        )

    def add(self, c: Customer) -> Customer:
        cur = self.db.execute(
            "INSERT INTO customers(name, email, country, region, vat_id, tax_exempt, currency, created_at)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (c.name, c.email, c.country, c.region, c.vat_id, int(c.tax_exempt), c.currency,
             c.created_at.isoformat()),
        )
        c.id = cur.lastrowid
        return c


class InvoiceRepository:
    def __init__(self, db: Database) -> None:
        self.db = db

    def _lines(self, invoice_id: int) -> list[LineItem]:
        rows = self.db.query_all(
            "SELECT * FROM line_items WHERE invoice_id = ? ORDER BY position", (invoice_id,)
        )
        return [
            LineItem(r["description"], Decimal(r["quantity"]), Decimal(r["unit_price"]),
                     r["tax_category"], bool(r["discountable"]))
            for r in rows
        ]

    def _from_row(self, row) -> Invoice:
        cur = row["currency"]
        return Invoice(
            id=row["id"], number=row["number"], customer_id=row["customer_id"], currency=cur,
            issue_date=_d(row["issue_date"]), due_date=_d(row["due_date"]),
            lines=self._lines(row["id"]), coupon_code=row["coupon_code"],
            status=InvoiceStatus(row["status"]),
            subtotal=from_minor(row["subtotal_minor"], cur),
            discount=from_minor(row["discount_minor"], cur),
            tax=from_minor(row["tax_minor"], cur),
            total=from_minor(row["total_minor"], cur),
            amount_paid=from_minor(row["paid_minor"], cur),
            reminders_sent=row["reminders_sent"],
            last_reminder_on=_d(row["last_reminder_on"]), notes=row["notes"],
        )

    def get(self, invoice_id: int) -> Invoice | None:
        row = self.db.query_one("SELECT * FROM invoices WHERE id = ?", (invoice_id,))
        return self._from_row(row) if row else None

    def list_open(self) -> list[Invoice]:
        rows = self.db.query_all(
            "SELECT * FROM invoices WHERE status IN (?, ?) ORDER BY due_date",
            (InvoiceStatus.ISSUED.value, InvoiceStatus.PARTIALLY_PAID.value),
        )
        return [self._from_row(r) for r in rows]

    def list_for_customer(self, customer_id: int) -> list[Invoice]:
        rows = self.db.query_all(
            "SELECT * FROM invoices WHERE customer_id = ? ORDER BY issue_date DESC", (customer_id,)
        )
        return [self._from_row(r) for r in rows]

    def save(self, inv: Invoice) -> Invoice:
        cur = inv.currency
        values = (
            inv.number, inv.customer_id, cur,
            inv.issue_date.isoformat() if inv.issue_date else None,
            inv.due_date.isoformat() if inv.due_date else None,
            inv.status.value, inv.coupon_code,
            to_minor(inv.subtotal, cur), to_minor(inv.discount, cur), to_minor(inv.tax, cur),
            to_minor(inv.total, cur), to_minor(inv.amount_paid, cur),
            inv.reminders_sent,
            inv.last_reminder_on.isoformat() if inv.last_reminder_on else None, inv.notes,
        )
        with self.db.transaction():
            if inv.id is None:
                c = self.db.execute(
                    "INSERT INTO invoices(number, customer_id, currency, issue_date, due_date, status,"
                    " coupon_code, subtotal_minor, discount_minor, tax_minor, total_minor, paid_minor,"
                    " reminders_sent, last_reminder_on, notes) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    values,
                )
                inv.id = c.lastrowid
            else:
                self.db.execute(
                    "UPDATE invoices SET number=?, customer_id=?, currency=?, issue_date=?, due_date=?,"
                    " status=?, coupon_code=?, subtotal_minor=?, discount_minor=?, tax_minor=?,"
                    " total_minor=?, paid_minor=?, reminders_sent=?, last_reminder_on=?, notes=?"
                    " WHERE id=?",
                    values + (inv.id,),
                )
                self.db.execute("DELETE FROM line_items WHERE invoice_id = ?", (inv.id,))
            for pos, line in enumerate(inv.lines):
                self.db.execute(
                    "INSERT INTO line_items(invoice_id, position, description, quantity, unit_price,"
                    " tax_category, discountable) VALUES (?,?,?,?,?,?,?)",
                    (inv.id, pos, line.description, str(line.quantity), str(line.unit_price),
                     line.tax_category, int(line.discountable)),
                )
        return inv


class PaymentRepository:
    def __init__(self, db: Database) -> None:
        self.db = db

    def add(self, p: Payment) -> Payment:
        cur = self.db.execute(
            "INSERT INTO payments(invoice_id, amount_minor, currency, received_on, method, reference)"
            " VALUES (?, ?, ?, ?, ?, ?)",
            (p.invoice_id, to_minor(p.amount, p.currency), p.currency, p.received_on.isoformat(),
             p.method, p.reference),
        )
        p.id = cur.lastrowid
        return p

    def for_invoice(self, invoice_id: int) -> list[Payment]:
        rows = self.db.query_all("SELECT * FROM payments WHERE invoice_id = ?", (invoice_id,))
        return [
            Payment(r["id"], r["invoice_id"], from_minor(r["amount_minor"], r["currency"]),
                    r["currency"], date.fromisoformat(r["received_on"]), r["method"], r["reference"])
            for r in rows
        ]
