## Decisions

- **Restart instead of arguing.** Appending "tools are off" to a tool-heavy history did not change the model's behaviour; removing the history and the tool declarations removes both the pattern and the template's tool-call instructions.
- **Findings as text**: each unique non-error tool result with the call that produced it (`read_file {"path": ...}`), newest first within 60k characters, then put back in call order. Errors carry no facts the artifact needs.
- **Only for read-only phases' forced replies** (the only place tools are forced off); write-capable phases abort on stall as before.

## Risks / Trade-offs

- [Findings past the cap are lost] → oldest go first; 60k characters is well inside the windows that reach this point.
