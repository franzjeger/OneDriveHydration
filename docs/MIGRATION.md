# Migration map

OneDriveForLinux is a donor and behavioral reference, not a base branch.

| Capability | Decision |
|---|---|
| FUSE VFS | Do not port |
| Existing sync/delta engine | Replace with HydrationAPI |
| Existing upload queue | Replace with HydrationAPI and GraphSink |
| Existing token cache | Replace with hydration-graph TokenCache |
| Device-code presentation | Port behind hydration-graph auth |
| PKCE browser flow | Adapt after threat-model review |
| Automatic drive discovery | Port Graph behavior, rewrite transport integration |
| QuickXorHash | Port algorithm and test vectors with attribution |
| Resumable downloads | Port behavior after the fetch seam streams/ranges |
| D-Bus, CLI, tray and flyout | Port as product-layer clients |
| systemd and installers | Adapt after daemon CLI stabilizes |
| Dolphin integration | Adapt to HydrationAPI status and xattrs |

Every port should be a small PR that states provenance, license, changed assumptions and tests.
