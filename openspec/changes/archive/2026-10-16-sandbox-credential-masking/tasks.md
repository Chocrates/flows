## 1. Sandbox

- [x] 1.1 `Confinement` + `BashConfig.expose_credentials`; `run_confined` takes `&Confinement`; callers (bash, plugin, tests) updated; plugin YAML key
- [x] 1.2 `sandbox::hidden_paths` (credential list, existing + canonicalized, flows secrets always); `Sandbox::wrap` takes the list; unit tests
- [x] 1.3 bwrap applies masks before repo/cache binds; real-sandbox tests (key hidden by abs path, flows config hidden with opt-out, opt-out exposes, repo under masked dir works)
- [x] 1.4 Seatbelt deny rules for the hidden paths (profile unit test; runtime verified on macOS later)

## 2. Surface + docs

- [x] 2.1 `flows doctor` credentials line; docs (templates.md bash fields, security notes); ROADMAP
