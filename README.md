# OneDriveHydration

A native OneDrive client for Linux built on HydrationAPI's fail-closed filesystem and
cloud-access invariants.

This repository is intentionally a new product shell rather than a continuation of the
FUSE sync engine in [OneDriveForLinux](https://github.com/franzjeger/OneDriveForLinux).
OneDriveForLinux remains the working reference client and a donor for product features;
HydrationAPI owns hydration, reconciliation, uploads, Graph delta state and the privileged
security boundary.

## Status

Early integration work. The daemon has device-code enrollment backed by Linux Secret Service,
automatic primary-drive discovery, reviewed GraphAccess wiring, constant-memory streamed
downloads and fail-closed QuickXorHash verification. Desktop integration, resumable ranges and
packaging are not yet complete. It is not ready for user data.

## Design rules

- The privileged helper never receives credentials and never opens network connections.
- Missing or inconsistent state fails closed; cursors never advance past unapplied changes.
- HydrationAPI is the only owner of delta, upload and reconciliation state.
- Code ported from OneDriveForLinux must be isolated behind the new interfaces and retain
  its original license and attribution.
- No live credential is required by unit or integration tests.

See [the architecture](docs/ARCHITECTURE.md), [migration map](docs/MIGRATION.md),
[security model](docs/SECURITY.md), and [roadmap](docs/ROADMAP.md).

## License

Licensed under either [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT), at your option.

## Current development invocation

Enroll once; the refresh token is stored in the desktop's Linux Secret Service collection.
The command fails closed when no Secret Service provider is available or the collection cannot
be unlocked; it never falls back to a plaintext token file.

On first start after upgrading from the file-backed alpha, an existing `refresh-token` file in
the state directory is migrated into Secret Service and removed only after the secure write
succeeds. A migration error stops startup.

```text
cargo run -p onedrive-hydration-daemon -- auth \
  --state-dir "$HOME/.local/state/onedrive-hydration" \
  --client-id <azure-client-id>
```

Then start the daemon. The signed-in user's primary drive ID is resolved automatically:

```text
cargo run -p onedrive-hydration-daemon -- run \
  --mount "$HOME/OneDrive" \
  --state-dir "$HOME/.local/state/onedrive-hydration" \
  --client-id <azure-client-id>
```

With the daemon running, query its owner-only control socket or safely return a hydrated file to
a placeholder:

```text
cargo run -p onedrive-hydration-daemon --bin onedrive-hydrationctl -- status
cargo run -p onedrive-hydration-daemon --bin onedrive-hydrationctl -- \
  evict "Documents/report.pdf"
```

Both commands use `$XDG_RUNTIME_DIR/onedrive-hydration.ctl`. If the runtime directory is not
available, pass an explicit `--socket`; the daemon and CLI do not fall back to a shared `/tmp`
path.
