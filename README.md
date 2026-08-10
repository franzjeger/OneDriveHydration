# OneDriveHydration

A native OneDrive client for Linux built on HydrationAPI's fail-closed filesystem and
cloud-access invariants.

This repository is intentionally a new product shell rather than a continuation of the
FUSE sync engine in [OneDriveForLinux](https://github.com/franzjeger/OneDriveForLinux).
OneDriveForLinux remains the working reference client and a donor for product features;
HydrationAPI owns hydration, reconciliation, uploads, Graph delta state and the privileged
security boundary.

## Status

Early integration work. The daemon is wired to the reviewed GraphAccess implementation,
but enrollment, automatic drive discovery, desktop integration and packaging are not yet
complete. It is not ready for user data.

## Design rules

- The privileged helper never receives credentials and never opens network connections.
- Missing or inconsistent state fails closed; cursors never advance past unapplied changes.
- HydrationAPI is the only owner of delta, upload and reconciliation state.
- Code ported from OneDriveForLinux must be isolated behind the new interfaces and retain
  its original license and attribution.
- No live credential is required by unit or integration tests.

See [the architecture](docs/ARCHITECTURE.md), [migration map](docs/MIGRATION.md),
[security model](docs/SECURITY.md), and [roadmap](docs/ROADMAP.md).

## Current development invocation

The current preflight deliberately refuses to start without an externally enrolled refresh
token:

```text
cargo run -p onedrive-hydration-daemon -- \
  --mount "$HOME/OneDrive" \
  --state-dir "$HOME/.local/state/onedrive-hydration" \
  --client-id <azure-client-id> \
  --drive-id <microsoft-graph-drive-id>
```
