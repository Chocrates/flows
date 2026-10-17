"""Regional tax rates.

Rates are keyed by country, optionally refined by region (US states, Canadian
provinces). EU business customers with a valid VAT id from a different member
state are reverse-charged (zero rate on the bill).
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date
from decimal import Decimal

from invoicing.models import Customer

SELLER_COUNTRY = "IE"

EU_MEMBERS = {
    "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR",
    "HU", "IE", "IT", "LV", "LT", "LU", "MT", "NL", "PL", "PT", "RO", "SK",
    "SI", "ES", "SE",
}


@dataclass(frozen=True)
class TaxRate:
    country: str
    region: str | None
    category: str
    rate: Decimal
    valid_from: date = date(2000, 1, 1)


RATES: list[TaxRate] = [
    TaxRate("IE", None, "standard", Decimal("0.23")),
    TaxRate("IE", None, "reduced", Decimal("0.135")),
    TaxRate("DE", None, "standard", Decimal("0.19")),
    TaxRate("DE", None, "reduced", Decimal("0.07")),
    TaxRate("FR", None, "standard", Decimal("0.20")),
    TaxRate("GB", None, "standard", Decimal("0.20")),
    TaxRate("CA", "ON", "standard", Decimal("0.13")),
    TaxRate("CA", "QC", "standard", Decimal("0.14975")),
    TaxRate("CA", None, "standard", Decimal("0.05")),
    TaxRate("US", "CA", "standard", Decimal("0.0725")),
    TaxRate("US", "NY", "standard", Decimal("0.04")),
    TaxRate("US", "TX", "standard", Decimal("0.0625")),
]


def is_reverse_charge(customer: Customer) -> bool:
    """B2B supply to another EU member state with a VAT id."""
    return (
        bool(customer.vat_id)
        and customer.country in EU_MEMBERS
        and customer.country != SELLER_COUNTRY
    )


def _candidates(country: str, region: str | None, category: str, on: date) -> list[TaxRate]:
    found = [
        r for r in RATES
        if r.country == country and r.category == category and r.valid_from <= on
        and (r.region is None or r.region == region)
    ]
    # most specific (region match) first, then newest
    return sorted(found, key=lambda r: (r.region is None, -r.valid_from.toordinal()))


def rate_for(customer: Customer, category: str, on: date | None) -> Decimal:
    """Applicable tax rate for a line of ``category`` sold to ``customer``."""
    if customer.tax_exempt or is_reverse_charge(customer):
        return Decimal("0")
    on = on or date.today()
    matches = _candidates(customer.country, customer.region, category, on)
    if not matches and category != "standard":
        matches = _candidates(customer.country, customer.region, "standard", on)
    return matches[0].rate if matches else Decimal("0")


def tax_note(customer: Customer) -> str | None:
    """Legal note printed on the document, if any."""
    if is_reverse_charge(customer):
        return "Reverse charge: VAT to be accounted for by the recipient."
    if customer.tax_exempt:
        return "Exempt from tax."
    return None
