## 1. Profile

- [x] 1.1 IP-only network rules + DNS/syslog socket exceptions; mach-lookup allowlist; tool-cache write; same-sandbox signals; profile unit tests
- [x] 1.2 macOS integration tests (unix socket, AppleEvents, toolchain + DNS/TLS); credential-masking tests enabled on macOS

## 2. Verification (on a Mac)

- [ ] 2.1 `make check` passes on macOS, including the `macos_sandbox_*` and credential tests
- [ ] 2.2 Manual escape probes fail: `open -a Calculator`, `osascript -e 'tell application "Terminal" to do script "touch ~/ESCAPED"'`, `launchctl submit -l x -- /usr/bin/touch ~/ESCAPED`
- [ ] 2.3 A real run with `files_and_test` (cargo/npm build that fetches dependencies) works; add any needed mach services to `MACH_SERVICES`
- [ ] 2.4 Update ROADMAP (remove the macOS parity item), archive the change, merge to main
