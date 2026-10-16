## Decisions

- **Name `offline`, default false:** `BashConfig` derives `Default`, so a `network: bool` defaulting to true would silently be false in code-built configs. The flag also reads naturally in YAML.
- **Linux:** `--unshare-net` gives a fresh network namespace with only `lo` up. That cuts the host's 127.0.0.1 too, which keeps host-loopback services (databases, dev servers, local LLM endpoints) unreachable.
- **macOS:** replace `(allow network*)` with `localhost`-only `network-outbound` / `network-inbound` / `network-bind` rules. Unlike Linux, macOS loopback is shared with the host, so offline commands can still reach host services on localhost. Name resolution can also go through mach services (libinfo → opendirectoryd). Both are weaker than Linux and documented. The `macos-seatbelt-ipc` change adds the mach allowlist and drops the DNS socket when offline.
- **Model hint:** an offline model that doesn't know it's offline burns rounds retrying `cargo fetch`/`npm install`, so the tool description says so up front.
