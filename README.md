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
downloads and fail-closed QuickXorHash verification, a D-Bus state service and tray, and a
validated systemd installer that refuses deployments which would fail open
(see [packaging/systemd](packaging/systemd/README.md)), and a Plasma flyout plasmoid with
eviction. The re-authentication UX and Dolphin integration are not yet built. It is not
ready for user data.

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

Then start the daemon. The signed-in user's primary drive ID is resolved automatically.
At startup `run` waits, bounded (60s), for `org.freedesktop.secrets` to be owned or
activatable on the session bus — at login the daemon is regularly started before PAM has
brought the credential store up (measured; the store here is `ksecretd`, started inside the
session scope, so no unit ordering can express the dependency) — and its errors distinguish
"the store is not up" from "there is no credential":

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

For desktop integration there is a session D-Bus service that mirrors the control socket, so a
tray can subscribe instead of polling and never needs to know the socket exists:

```text
cargo run -p onedrive-hydration-daemon --bin onedrive-hydration-dbus
```

It owns `io.github.franzjeger.OneDriveHydration` and serves, at the object path of the same
name, `DaemonRunning`, `Unsent`, `Excluded` and `Exposures` properties, an `Evict(path)`
method with named errors, and a `StateChanged` signal that fires once per distinct state.
While the daemon is down the service keeps running and reports `DaemonRunning` false; when the
daemon restarts it reconnects on its own with bounded backoff. Eviction over the bus is held
to the same boundary as the socket: callers whose uid the bus cannot attribute to the daemon's
owner are refused. Installed deployments never start this service eagerly: the installer
writes a D-Bus activation file, and the session bus starts the service the first time
anything talks to the name (see [packaging/systemd](packaging/systemd/README.md)).

The tray icon subscribes to exactly that signal — it never polls:

```text
cargo run -p onedrive-hydration-daemon --bin onedrive-hydration-tray -- \
  --mount "$HOME/OneDrive"
```

It is a StatusNotifierItem with a DBusMenu, spoken directly over zbus with no GUI toolkit:
the panel draws everything. Four states are shown, in order of precedence: daemon (or state
service) not running, another mount exposing the sync files (`Exposures > 0`, rendered as
`NeedsAttention` because reads through such a mount bypass hydration), changes waiting to
upload, and up to date. Icons resolve by name from the hicolor theme; run
`packaging/icons/install-icons.sh` once per user to install them. On a desktop with no
`org.kde.StatusNotifierWatcher` the binary exits saying so, and when the watcher restarts —
plasmashell and kded6 do — it re-registers by itself. Eviction is deliberately absent from
the menu: it needs a file picker, which needs a toolkit, which is the flyout's decision to
make.

On Plasma 6 the flyout exists, and it made the opposite trade the same way: a plasmoid —
QML loaded by plasmashell's system tray, shipped as data with zero new Rust dependencies —
instead of a toolkit. Install it per user with `packaging/plasmoid/install-plasmoid.sh`
(icons first, as above); the running shell adopts it into the system tray by itself. It
subscribes to the same `StateChanged` signal, shows the same states with the same wording —
a test pins the two surfaces together — and adds the two actions the tray could not draw:
opening the sync folder, and "Free Up Space…", which picks a file under the mount and calls
`Evict`, quoting the daemon's refusal reason verbatim when it declines. On Plasma the
plasmoid *is* the tray presence; running the SNI binary alongside it shows a second icon,
so keep that one for desktops without plasmashell. What the flyout does not show is what
the D-Bus surface cannot yet say — account, quota, per-file transfers, byte totals,
credential health — and `packaging/plasmoid/README.md` keeps that list honestly.
