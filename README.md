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
downloads and fail-closed QuickXorHash verification, a D-Bus state service and tray, a
validated systemd installer that refuses deployments which would fail open
(see [packaging/systemd](packaging/systemd/README.md)), and a Plasma flyout plasmoid with
eviction. The daemon says whether it is signed in — a credential state on its own
socket and on the D-Bus surface, shown by the tray and flyout with the enrollment
instruction that works here — and, with the PKCE threat-model review accepted
(2026-08-13), it can now enroll itself: `auth --browser` runs the authorization-code +
PKCE flow in-product, straight into Secret Service with no plaintext moment, and
restarts a running daemon onto the new sign-in. Dolphin has the action half of its
integration ("Free Up Space", shipped as data); the status overlays are not built,
because they are the one surface with no data-only path. It is not ready for user
data.

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

On a tenant whose Conditional Access blocks the device code flow — this deployment's does —
pass `--browser`: the daemon runs the authorization-code + PKCE flow itself, on a loopback
listener bound once at `127.0.0.1`, and installs the result directly into Secret Service
(the accepted `docs/PKCE-ENROLLMENT-REVIEW.md` is the design record). `--no-open` prints
the sign-in URL instead of launching `xdg-open`. `--browser` deliberately skips the
"already signed in" short-circuit, because it is also the re-enrollment path — and after
storing the credential it `try-restart`s `onedrive-hydration.service`, so a daemon running
signed-out picks the new sign-in up without manual intervention. `tools/pkce-enroll.py`
remains as the out-of-product fallback; its file hand-off and the adopt-on-restart
migration still work exactly as described below.

At every start, a `refresh-token` file in the state directory — the file-backed alpha's, or
one freshly written by `tools/pkce-enroll.py` — is adopted: written into Secret Service,
*replacing* any stored credential, and removed only after the secure write succeeds. The file
wins on purpose: the daemon consumes it on every start, so its presence means an enrollment
happened since the last start, and the one situation that produces both at once is a stored
credential the service has rejected plus the fresh sign-in that fixes it. A migration error
stops startup. While running signed-out (the service refused the stored credential), the
daemon watches for that file and, once it has settled, exits so its systemd unit restarts it
onto the new sign-in — enrollment while the daemon runs therefore needs no manual restart.

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
It also serves `CredentialState` — `healthy`, `unsaved` (syncing works but the rotated
sign-in cannot be written to Secret Service), `rejected` (the service has conclusively
refused the stored sign-in), or `unknown` when no running daemon has asserted one — with a
`CredentialStateChanged` signal of its own; a new argument on `StateChanged` would have
broken subscribers that decode it by signature, so the contract grows by new members only,
and readers treat unrecognised `CredentialState` values as `unknown`. The conclusion comes
from the daemon's second owner-only socket (`onedrive-hydration.auth`, same line protocol
as the control socket), where `onedrive-hydrationctl status` also reads it. While the
daemon is down the service keeps running and reports `DaemonRunning` false — and
`CredentialState` returns to `unknown` rather than being held, because a sign-in
instruction backed by a dead process is the wrong message: a stopped daemon cannot tell a
missing credential from a keyring that merely has not unlocked yet. When the daemon
restarts it reconnects on its own with bounded backoff. Eviction over the bus is held
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
the panel draws everything. Five states are shown, in order of precedence: daemon (or state
service) not running, another mount exposing the sync files (`Exposures > 0`, rendered as
`NeedsAttention` because reads through such a mount bypass hydration), sign-in required
(`CredentialState` `rejected`, also `NeedsAttention`: nothing is lost, and the tooltip
names `onedrive-hydration-daemon auth --browser` because Conditional Access blocks the
device-code flow on this deployment — deliberately a sentence and not a button, since the
tray cannot run a browser flow in its own process), changes waiting to upload, and up to
date. An `unsaved` credential is
a warning sentence appended to whichever of the running states is shown, not a state of
its own: syncing still works, and the sentence says what breaks (the next restart) and
what to do (unlock the keyring). Icons resolve by name from the hicolor theme; run
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
so keep that one for desktops without plasmashell. Which of the two a deployment installs
is told to the installer — `--tray sni|plasmoid|none` — and never detected: the applet
draws only under plasmashell, the binary wherever there is a `StatusNotifierWatcher`, and
which desktop the user logs into is not a fact at install time. Say nothing and it defaults
to the binary, unless the applet is already installed for that user, which is refused until
one of the three is named. What the flyout does not show is what
the D-Bus surface cannot yet say — account, quota, per-file transfers, byte totals,
credential health — and `packaging/plasmoid/README.md` keeps that list honestly.

Dolphin gets the same trade a third time: "Free Up Space" on a selected file is a KIO
servicemenu — a `.desktop` file and a shell wrapper, no toolkit — installed per user with
`packaging/dolphin/install-servicemenu.sh`. That KIO cannot filter a menu entry by path
was measured, not assumed, so the entry exists on every file and the wrapper refuses,
naming the sync root, for anything outside it; and because `onedrive-hydrationctl` exits
zero when the daemon *declines* an eviction, the wrapper reads the reply rather than the
exit status, with a test deriving those reply prefixes from the Rust parser. The status
overlays are the exception to the whole pattern: `KOverlayIconPlugin` is compiled C++ with
no data-only equivalent, so it is a dependency decision rather than more of the same, and
`docs/DOLPHIN-GROUNDWORK.md` sets out what it would cost and what it must not get wrong.
