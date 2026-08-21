# Migration map

OneDriveForLinux is a donor and behavioral reference, not a base branch.

| Capability | Decision |
|---|---|
| FUSE VFS | Do not port |
| Existing sync/delta engine | Replace with HydrationAPI |
| Existing upload queue | Replace with HydrationAPI and GraphSink |
| Existing token cache | Replace with hydration-graph TokenCache |
| Device-code presentation | Port behind hydration-graph auth |
| PKCE browser flow | Rewritten in Rust after threat-model acceptance; retained loopback listener, S256/state validation, direct Secret Service write |
| Automatic drive discovery | Port Graph behavior, rewrite transport integration |
| QuickXorHash | Implemented in HydrationAPI from Microsoft's published algorithm; no donor code |
| Resumable downloads | Port behavior after the fetch seam streams/ranges |
| D-Bus, CLI, tray and flyout | Port as product-layer clients |
| systemd and installers | Adapt after daemon CLI stabilizes |
| Dolphin integration | Adapt to HydrationAPI status and xattrs |

Every port should be a small PR that states provenance, license, changed assumptions and tests.
