## Why

On Linux, a sandboxed command could reach the user's session bus over a unix socket and run code outside the sandbox. The seccomp filter closed that (d12aafe). The macOS Seatbelt profile still has `(allow network*)`, which includes unix-socket connects, and a bare `(allow mach-lookup)`, which reaches every launchd service, including LaunchServices and AppleEvents. Those can start programs outside the sandbox (`open -a Terminal`, `osascript`). The same class of escape very likely exists on macOS.

## What Changes

- Seatbelt network rules become IP-only (`remote ip` / `local ip`). Unix-socket connects are denied except the mDNSResponder (DNS) and syslog sockets.
- `mach-lookup` becomes an allowlist (`MACH_SERVICES`) covering user/group lookup, preferences, logging, TLS trust evaluation, DNS/network configuration, and FSEvents.
- Fixes a latent macOS bug: writes to the toolchain cache (`CARGO_HOME`, npm/pip caches) are now allowed, as they already are on Linux.
- With `tools.bash.offline`, the IP rules narrow to `localhost` and the DNS socket is dropped.
- Signals may target any process in the same sandbox (`make -j`, test runners), not only the shell itself.
- macOS-only integration tests: unix socket blocked, AppleEvents blocked, cargo/git/DNS/TLS still working, and the credential-masking tests now run on macOS too.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `security-model`: "No host IPC from the sandbox" now covers macOS; the "known gap" scenario is replaced.

## Impact

- `crates/flows-tools/src/sandbox/seatbelt.rs` profile + tests; `crates/flows-tools/tests/sandbox.rs` macOS tests.
- Needs verification on a Mac before merge. A missing mach service shows up as a tool failing inside the sandbox.
