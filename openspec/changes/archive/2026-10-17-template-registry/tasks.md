## 1. Core

- [x] 1.1 `registry.rs`: source parse, fetch (git), package discovery, review, lockfile, install/update/remove/trust, name checks, diff
- [x] 1.2 `PhaseConfig.untrusted`; mark at template/project load (root + includes)

## 2. Confinement

- [x] 2.1 `sandbox::confined_command`; script phases of untrusted templates run confined; agent ignores expose_credentials when untrusted
- [x] 2.2 `flows plan` marks confined phases

## 3. CLI + tests + docs

- [x] 3.1 `templates install|installed|update|remove|trust`
- [x] 3.2 Tests with local git repos: install/pin/review/decline/collision/ambiguous/update diff/remove/trust; confined script can't write outside repo; trusted runs on host
- [x] 3.3 docs/templates.md, DESIGN.md, ROADMAP; make check
