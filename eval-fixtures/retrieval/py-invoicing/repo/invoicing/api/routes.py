"""Flask application exposing the billing API."""

from __future__ import annotations

from datetime import date

from flask import Flask, Response, g, jsonify, request

from invoicing import payments, tabular
from invoicing.api import serializers as ser
from invoicing.api.guard import require
from invoicing.mailer import Mailer
from invoicing.pdf import render_pdf
from invoicing.service import BillingService, LifecycleError
from invoicing.settings import get_config
from invoicing.storage.db import Database, open_database
from invoicing.storage.repository import CustomerRepository, InvoiceRepository, PaymentRepository

app = Flask(__name__)
_db: Database | None = None


def get_db() -> Database:
    global _db
    if _db is None:
        _db = open_database(get_config().database_path)
    return _db


def _service() -> BillingService:
    return BillingService(get_db(), Mailer())


@app.errorhandler(ser.ValidationError)
def _bad_request(exc):
    return jsonify(error=str(exc)), 400


@app.get("/customers/<int:customer_id>")
@require("read", get_db)
def get_customer(customer_id: int):
    c = CustomerRepository(get_db()).get(customer_id)
    if c is None:
        return jsonify(error="not found"), 404
    return jsonify(ser.customer_to_json(c))


@app.post("/invoices")
@require("write", get_db)
def create_invoice():
    body = request.get_json(force=True)
    lines = ser.parse_lines(body.get("lines"))
    inv = _service().create_draft(int(body["customer_id"]), lines,
                                  coupon_code=body.get("coupon_code"), actor=g.principal.label)
    return jsonify(ser.invoice_to_json(inv)), 201


@app.get("/invoices/<int:invoice_id>")
@require("read", get_db)
def get_invoice(invoice_id: int):
    inv = InvoiceRepository(get_db()).get(invoice_id)
    if inv is None:
        return jsonify(error="not found"), 404
    return jsonify(ser.invoice_to_json(inv))


@app.post("/invoices/<int:invoice_id>/issue")
@require("write", get_db)
def issue_invoice(invoice_id: int):
    try:
        inv = _service().issue(invoice_id, actor=g.principal.label)
    except LifecycleError as exc:
        return jsonify(error=str(exc)), 409
    return jsonify(ser.invoice_to_json(inv))


@app.post("/invoices/<int:invoice_id>/payments")
@require("write", get_db)
def add_payment(invoice_id: int):
    body = request.get_json(force=True)
    db = get_db()
    try:
        p = payments.record_payment(
            InvoiceRepository(db), PaymentRepository(db), invoice_id,
            ser._decimal(body.get("amount"), "amount"), body.get("currency", ""),
            ser.parse_date(body.get("received_on", date.today().isoformat()), "received_on"),
            method=body.get("method", "bank_transfer"), reference=body.get("reference"),
            actor=g.principal.label,
        )
    except payments.PaymentError as exc:
        return jsonify(error=str(exc)), 422
    return jsonify(ser.payment_to_json(p)), 201


@app.get("/invoices/<int:invoice_id>/pdf")
@require("read", get_db)
def invoice_pdf(invoice_id: int):
    db = get_db()
    inv = InvoiceRepository(db).get(invoice_id)
    if inv is None:
        return jsonify(error="not found"), 404
    customer = CustomerRepository(db).get(inv.customer_id)
    return Response(render_pdf(inv, customer), mimetype="application/pdf")


@app.get("/exports/invoices.csv")
@require("export", get_db)
def export_invoices():
    start = ser.parse_date(request.args["from"], "from")
    end = ser.parse_date(request.args["to"], "to")
    rows = get_db().query_all("SELECT id FROM invoices")
    repo = InvoiceRepository(get_db())
    invoices = tabular.in_period([repo.get(r["id"]) for r in rows], start, end)
    return Response(tabular.invoices_csv(invoices), mimetype="text/csv")
