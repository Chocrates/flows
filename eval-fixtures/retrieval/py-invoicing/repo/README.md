# invoicing

A compact billing backend: customers, bills with line items, regional tax,
promo codes, payment recording, overdue reminders and exports.

## Running

```bash
pip install -e .[dev]
export INVOICING_CONFIG=./config.yml
flask --app invoicing.api.routes run
```

The reminder scheduler runs in-process; start it with
`python -m invoicing.jobs`.

## Configuration

Configuration is read from a YAML file and may be overridden by
`INVOICING_*` environment variables. See `invoicing/settings.py`.

## Tests

```bash
pytest
```
