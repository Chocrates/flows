"""Printable document rendering via reportlab."""

from __future__ import annotations

from io import BytesIO

from reportlab.lib.pagesizes import A4
from reportlab.lib.units import mm
from reportlab.pdfgen import canvas

from invoicing.billing import rules
from invoicing.models import Customer, Invoice
from invoicing.money import format_amount, quantize
from invoicing.settings import get_config

LEFT = 20 * mm
RIGHT = 190 * mm


def _header(c: canvas.Canvas, inv: Invoice, customer: Customer, top: float) -> float:
    cfg = get_config()
    c.setFont("Helvetica-Bold", 16)
    c.drawString(LEFT, top, cfg.company_name)
    c.setFont("Helvetica", 9)
    c.drawString(LEFT, top - 12, cfg.company_address)
    c.setFont("Helvetica-Bold", 12)
    c.drawRightString(RIGHT, top, f"Bill {inv.number or 'DRAFT'}")
    c.setFont("Helvetica", 9)
    c.drawRightString(RIGHT, top - 12, f"Issued {inv.issue_date}  Due {inv.due_date}")
    y = top - 40
    c.drawString(LEFT, y, f"To: {customer.name}")
    if customer.vat_id:
        c.drawString(LEFT, y - 11, f"VAT: {customer.vat_id}")
    return y - 30


def _lines(c: canvas.Canvas, inv: Invoice, y: float) -> float:
    c.setFont("Helvetica-Bold", 9)
    for x, label in ((LEFT, "Description"), (120 * mm, "Qty"), (145 * mm, "Price"), (RIGHT, "Amount")):
        (c.drawRightString if x == RIGHT else c.drawString)(x, y, label)
    c.setFont("Helvetica", 9)
    for line in inv.lines:
        y -= 13
        amount = quantize(line.quantity * line.unit_price, inv.currency)
        c.drawString(LEFT, y, line.description[:60])
        c.drawString(120 * mm, y, str(line.quantity))
        c.drawString(145 * mm, y, str(line.unit_price))
        c.drawRightString(RIGHT, y, format_amount(amount, inv.currency))
    return y - 20


def _summary(c: canvas.Canvas, inv: Invoice, y: float) -> float:
    rows = [("Subtotal", inv.subtotal), ("Discount", -inv.discount), ("Tax", inv.tax),
            ("Total", inv.total), ("Paid", -inv.amount_paid), ("Balance due", inv.balance)]
    for label, value in rows:
        c.drawString(140 * mm, y, label)
        c.drawRightString(RIGHT, y, format_amount(value, inv.currency))
        y -= 12
    return y


def render_pdf(inv: Invoice, customer: Customer) -> bytes:
    buf = BytesIO()
    c = canvas.Canvas(buf, pagesize=A4)
    _, height = A4
    y = _header(c, inv, customer, height - 25 * mm)
    y = _lines(c, inv, y)
    y = _summary(c, inv, y)
    note = rules.tax_note(customer)
    c.setFont("Helvetica-Oblique", 8)
    if note:
        c.drawString(LEFT, 30 * mm, note)
    c.drawString(LEFT, 20 * mm, get_config().bank_details)
    c.showPage()
    c.save()
    return buf.getvalue()
