## 1. Core

- [x] 1.1 `PhaseConfig.origin`; `include.rs` expander (resolve, nest, cycle/depth, alias rewrite, with/vars, when/depends_on, DAG wiring, duplicate check)
- [x] 1.2 Wire into `load_template` and `load_project_config`; `parse_phases` rejects includes without a resolver; duplicate-name check in `validate_pipeline`
- [x] 1.3 `eval_when` takes vars + scoped artifacts; scoped view helper

## 2. Runner

- [x] 2.1 Prompt lookup by origin; `vars` + scoped `prior_artifacts` in agent prompt ctx; `vars` in script command ctx; scoped artifacts for `when`

## 3. Tests & docs

- [x] 3.1 Core tests: splice order, nesting, cycle, clash, alias rewrite, vars, include when/depends_on, DAG chaining, DAG-in-linear error
- [x] 3.2 Runner tests: included script phase renders vars; aliased when reads short name; included prompt lookup
- [x] 3.3 docs/templates.md, DESIGN.md, ROADMAP; `make check`
