## Why

Sandboxed commands share the host network so builds can fetch dependencies. For phases that don't need it (running tests, linting, reviewing untrusted code), the network is pure exfiltration surface. The roadmap called for an opt-in way to cut it.

## What Changes

- `tools.bash.offline: true` (and `offline: true` in plugin YAML) runs commands with loopback only: bwrap `--unshare-net`; Seatbelt allows only `localhost` IP traffic.
- The `bash` tool description tells the model it has no network, so it doesn't try to fetch dependencies.
- Default unchanged (network on).

## Capabilities

### Modified Capabilities
- `security-model`: new requirement for opt-in network isolation.

## Impact

`BashConfig.offline`, `sandbox::Confinement.offline`, both backends, plugin YAML, agent tool description, docs.
