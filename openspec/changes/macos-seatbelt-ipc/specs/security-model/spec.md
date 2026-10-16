## MODIFIED Requirements

### Requirement: No host IPC from the sandbox

The system SHALL prevent sandboxed commands from reaching host services that could run code outside the sandbox, while still allowing IP networking so builds, package fetches, and in-process IPC keep working. A read-only filesystem does not stop a process from connecting to a unix socket or a launchd service, and host services reachable that way (the systemd/D-Bus session bus, container engines, the display server, abstract-namespace sockets, macOS LaunchServices and AppleEvents) can start programs unconfined.

On Linux the system SHALL deny creation of unix (`AF_UNIX`) sockets and io_uring instances with a seccomp filter, and deny syscalls made through a foreign ABI, while allowing `socketpair()` and inet sockets.

On macOS the Seatbelt profile SHALL allow network traffic only to and from IP endpoints, SHALL deny unix-socket connects except to the system DNS resolver and syslog sockets, and SHALL allow `mach-lookup` only for an explicit allowlist of system services (directory/user lookup, preferences, logging, TLS trust evaluation, DNS/network configuration, file events). Services that launch or control other programs SHALL NOT be on the allowlist.

#### Scenario: Session-bus escape attempt

- **WHEN** a sandboxed command on Linux tries to reach the user's session bus (e.g. `systemd-run --user …`) or any other unix socket on the host
- **THEN** socket creation fails with a permission error and nothing runs outside the sandbox.

#### Scenario: Builds and IPC still work

- **WHEN** a sandboxed command fetches dependencies over the network or uses `socketpair()`-based IPC
- **THEN** it succeeds as before.

#### Scenario: macOS unix socket denied

- **WHEN** a sandboxed command on macOS connects to a unix socket the host is listening on
- **THEN** the connect is denied and the host never sees a connection.

#### Scenario: macOS app control denied

- **WHEN** a sandboxed command on macOS tries to drive another application (e.g. `osascript -e 'tell application "Finder" …'`, `open -a …`)
- **THEN** the request fails and nothing runs outside the sandbox.

#### Scenario: macOS builds still resolve and fetch

- **WHEN** a sandboxed command on macOS runs the toolchain (`cargo`, `git`) or fetches over HTTPS
- **THEN** name resolution, TLS certificate validation, and toolchain cache writes work.
