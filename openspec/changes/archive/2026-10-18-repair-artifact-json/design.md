## Decisions

- **Hand-rolled balancer, no new dependency.** Three structural slips cover what was seen; strings and escapes are tracked so brackets in text are ignored.
- **Repair only on parse failure**, so valid JSON is never altered.
- **Envelope check unchanged**: a repaired candidate must still be `{"type": "artifact" | "questions", ...}`, so prose with stray braces is not turned into an artifact.
