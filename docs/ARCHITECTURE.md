# Architecture

The product is split at the credential and privilege boundary.

```text
desktop UI / CLI / D-Bus
           |
unprivileged OneDrive daemon
  GraphAccess -> fetch / upload / delta
  one shared TokenCache
           |
HydrationAPI reconciliation and queues
           |
narrow local protocol
           |
privileged hydrationd (no token, no network)
           |
ordinary Linux filesystem
```

HydrationAPI remains an external, revision-pinned dependency. This repository must not grow
a second delta cursor, upload queue, conflict engine or token cache. Product state such as UI
preferences may use its own store, but cloud truth and filesystem truth have one owner.

OneDriveForLinux's FUSE VFS is not part of this architecture. Product-facing components may
be ported after their assumptions about paths, xattrs and daemon control have been replaced
with HydrationAPI contracts.
