## Why

A 27B local model's 5.6k-character plan artifact failed to parse: it left the `files` array open and went on with the next key (`[{...}, "risks": [...]`) and closed one brace short. The phase stored it as `raw_output`, so the next phase got the plan as an escaped string instead of structured fields.

## What Changes

- When a reply's JSON candidate does not parse, the agent tries once more after balancing its brackets: closers that skip over open brackets close them first, closers with no opener are dropped, an array that is followed by `"key":` is closed before the comma that led to the key, and brackets still open at the end are closed. Text inside strings is untouched.
- The repaired value is accepted only if it parses and is an artifact or questions envelope, as before.

## Capabilities

### New Capabilities

### Modified Capabilities
- `model-io`: artifact JSON with unbalanced brackets is repaired.

## Impact

- `flows-runner::agent` (`parse_response`).
