## Context

The Seatbelt profile was written for filesystem confinement only: deny by default, allow reads, restrict writes, and allow `network*` and all `mach-lookup` so tools work. Linux closed host IPC with seccomp. Seatbelt has no syscall filter, but SBPL can filter both network operations and mach service lookups.

## Decisions

- **IP-only network:** `(allow network-outbound (remote ip "*:*"))`, `(allow network-inbound (local ip "*:*"))`, `(allow network-bind (local ip "*:*"))`, plus `(allow system-socket)` for routing/config sockets. Unix-socket connects fall through to `(deny default)`, except `/private/var/run/mDNSResponder`, which `getaddrinfo` needs, and `/private/var/run/syslog`.
- **Mach allowlist:** the services the sandboxes of other CLI agents (Codex, Chromium helpers) allow for command-line work. That means libinfo/membership (`getpwuid`), cfprefsd, logd/diagnosticd/system.logger, notification_center, dirhelper (`confstr` temp dirs), SecurityServer/trustd/ocspd (TLS), configd/DNSConfiguration/networkd/dnssd (DNS), and FSEvents (watchers). LaunchServices (`com.apple.coreservices.launchservicesd`, `com.apple.lsd.*`), AppleEvents (`com.apple.coreservices.appleevents`), pasteboard and WindowServer stay denied.
- **Tool-cache write:** Linux binds `tool_cache_dir()` read-write. On macOS `CARGO_HOME` etc. already pointed there, but writes were denied. Allowing the subpath brings the two backends to parity.
- **Unverified here:** this repo's development host is Linux. The profile is unit-tested as text, and the module was type-checked for `aarch64-apple-darwin` in isolation. The behavioural tests are `#[cfg(target_os = "macos")]` and must pass on a Mac before merge.

## Risks / Trade-offs

- A tool that needs an unlisted mach service will fail or degrade inside the sandbox, e.g. Xcode toolchains (`xcrun`) or keychain-backed git credential helpers. That's the intended direction (the keychain holds credentials); the fix is to add an entry after verifying it can't start programs.
- Denying unix sockets also blocks in-sandbox unix-socket servers, e.g. a test suite that binds one in `/tmp`. Linux has the same restriction.
