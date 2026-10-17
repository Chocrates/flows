## Decisions

- **Flatten at load time** (`flows_core::include`). Raw YAML entries are walked; `include` entries are resolved by a closure (`name -> yaml text`) so `flows-core` stays testable and both `load_template` and `load_project_config` share it. `parse_phases` without a resolver rejects includes with a clear error.
- **`PhaseConfig.origin`** (`template`, `local_name`, `prefix`, `vars`; serde-default, skipped when absent) carries what runtime needs: prompt lookup by origin template + local name, `vars` in render contexts, and the alias prefix for the short-name artifact view. Run meta stays backward compatible.
- **Alias separator `.`** reads naturally (`final.lint`). Artifacts are files named after the phase; dots are safe in file names. Jinja access to dotted keys needs `artifacts['final.lint']`, which is why the short-name view exists.
- **Short-name view**: for a phase with prefix `p`, every artifact `p<x>` is also exposed as `<x>`; scoped names win over an unrelated top-level phase of the same short name.
- **Include `when`** is ANDed as `(outer) and (inner)`; the outer expression is written in the parent's scope, which the short-name view doesn't hide.
- **DAG wiring per level**: a level is a DAG if any of its phases or include entries has `depends_on`. Linear includes are chained; roots of DAG includes get the entry's `depends_on`; group labels in `depends_on` expand to all group phase names. Phase names win over group labels.
- Depth cap 8 guards pathological nesting; cycles are detected by the resolution stack.
