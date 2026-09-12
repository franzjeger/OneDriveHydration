# OneDriveHydration

A native OneDrive client for Linux built on HydrationAPI's fail-closed filesystem and
cloud-access invariants.

This repository is intentionally a new product shell rather than a continuation of the
FUSE sync engine in [OneDriveForLinux](https://github.com/franzjeger/OneDriveForLinux).
OneDriveForLinux remains the working reference client and a donor for product features;
HydrationAPI owns hydration, reconciliation, uploads, Graph delta state and the privileged
security boundary.

## Status

Release candidate with a working sync foundation and an evolving desktop interface.
The product includes browser/PKCE and device-code
enrollment backed directly by Linux Secret Service, automatic primary-drive discovery,
streamed and resumable downloads, the fail-closed HydrationAPI sync engine, a validated
systemd installer, a signal-driven D-Bus service and tray, an in-product Plasma flyout with
sign-in and eviction, and Dolphin actions plus live file-status overlays. Tagged builds
publish a revision-matched, checksummed payload containing every runtime binary and desktop
asset.

Production readiness still requires external validation: the complete two-device/process-restart
matrix in the
[sync correctness gate](docs/SYNC-ACCEPTANCE.md) must pass against a dedicated,
non-production Microsoft 365 tenant. Until that evidence exists, treat this as a release
candidate rather than entrusting it with the only copy of user data.

The Plasma activity center shows account/quota, pause/resume, the actual upload queue,
confirmed upload history and unresolved sync issues. Dolphin has scoped context menus,
metadata-only status icons and availability jobs with progress and cancellation. The
remaining setup and sync-acceptance work is tracked in the roadmap.

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

At every start, a legacy `refresh-token` file in the state directory is migrated into Secret
Service and removed only after the secure write succeeds. This compatibility path exists for
file-backed alpha installations; current enrollment never writes plaintext credentials.
Browser enrollment stores the refresh token directly, then sends an owner-only restart
notification so a running daemon adopts the new account without a manual restart.

```text
cargo run -p onedrive-hydration-daemon -- auth \
  --state-dir "$HOME/.local/state/onedrive-hydration" \
  --client-id <azure-client-id>
```

Browser/PKCE is the default. Use `reauth` to replace an existing credential,
`--no-browser` to print the URL without launching it, or `--device-code` for a
headless machine whose tenant permits device-code sign-in.

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
cargo run -p onedrive-hydration-daemon --bin onedrive-hydration-dbus -- \
  --client-id <azure-client-id>
```

It owns `io.github.franzjeger.OneDriveHydration` and serves, at the object path of the same
name, `DaemonRunning`, `Unsent`, `Excluded`, `Exposures`, `Downloading`, `Indexing`, and
`Uploading` properties; owner-checked `Evict(path)`, `BeginEnrollment`, and
`EnrollmentStatus` methods; and signals that fire once per distinct daemon state.
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
the panel draws everything. The status prioritizes an unavailable or stopped service,
unsafe extra mounts,
sign-in requirements, active transfers/cloud checks, queued changes, then up to date.
Active work has a distinct sync icon; queued work does not imply upload progress.
An unsaved credential adds a short instruction to unlock the keyring. The menu's
"Hide tray icon (sync continues)" action closes only the tray process.
Icons resolve by name from the hicolor theme; run
`packaging/icons/install-icons.sh` once per user to install them. On a desktop with no
`org.kde.StatusNotifierWatcher` the binary exits saying so, and when the watcher restarts —
plasmashell and kded6 do — it re-registers by itself. Eviction is deliberately absent from
the menu: it needs a file picker, which needs a toolkit, which is the flyout's decision to
make.

On Plasma 6 the flyout exists, and it made the opposite trade the same way: a plasmoid —
QML loaded by plasmashell's system tray, shipped as data with zero new Rust dependencies —
instead of a toolkit. Install it per user with `packaging/plasmoid/install-plasmoid.sh`
(icons first, as above); the running shell adopts it into the system tray by itself. It
subscribes to the same state signals, shows the same states with the same wording — a test
pins the two surfaces together — and adds actions the tray could not draw: secure browser
sign-in, opening the sync folder, and "Free Up Space…", which picks a file under the mount
and calls `Evict`, quoting the daemon's refusal reason verbatim when it declines. On Plasma the
plasmoid *is* the tray presence; running the SNI binary alongside it shows a second icon,
so keep that one for desktops without plasmashell. Which of the two a deployment installs
is told to the installer — `--tray sni|plasmoid|none` — and never detected: the applet
draws only under plasmashell, the binary wherever there is a `StatusNotifierWatcher`, and
which desktop the user logs into is not a fact at install time. Say nothing and it defaults
to the binary, unless the applet is already installed for that user, which is refused until
one of the three is named. The flyout also reads the versioned `DesktopState` snapshot
and listens for desktop/job updates; see `packaging/plasmoid/README.md` for its limits.

Dolphin receives scoped "Free Up Space" and "Keep on Device" actions from a compiled
KF6 plugin. It accepts mixed file/folder selections only inside configured sync roots and
starts availability jobs through D-Bus. A separate overlay plugin reads xattr metadata
without opening file contents and refreshes visible status after changes. Standalone
`.desktop` menus and shell wrappers remain available as a fallback.

## Release payload

Tagged builds publish `onedrive-hydration-linux-x86_64.tar.gz` and its SHA-256 file. The
archive has a `/usr/local`-shaped `bin/` and `share/` tree, a per-file manifest, the exact
HydrationAPI revision, all six runtime binaries, documentation, and desktop integration.
After verifying the archive checksum, copy its contents into `/usr/local`, then run:

```text
sudo /usr/local/bin/onedrive-hydration-install install \
  --user "$USER" --mount "$HOME/OneDrive" --client-id <azure-client-id> \
  --tray plasmoid
```

The installer performs the kernel, filesystem, mount-namespace, exposure, fstab, Secret
Service, and payload checks before writing units. See
[packaging/systemd](packaging/systemd/README.md) for storage preparation and refusal details.

### Complete Plasma desktop installation

After installing the matching daemon binaries, run
`packaging/install-desktop.sh --mount ~/OneDrive --bin-dir /usr/local/bin` as the
session user. It installs the panel, icons and both Dolphin plugins, removes the
static fallback menus to avoid duplicates, and checks the installed components.
Use the same command with `--check` to diagnose a deployment. Restart Dolphin to
load new compiled plugins. The installer needs the KF6 build toolchain and
`kdialog` for fallback action dialogs.

The panel now reads the daemon's connected folder, account/quota snapshot, actual
upload queue (including last error and retry delay), and persistent confirmed
upload outcomes. Pause lasts two hours or until Resume/restart; an executing
background pass can finish, and explicit file reads still download. Retry retains
the sync engine's conditional-write checks. Availability jobs started from Dolphin
appear in the panel, including mixed file/folder selections, per-file failures,
processed-file progress and cancellation after the current file. Their byte count
measures local storage made available/reclaimed, not network throughput.

A filled green circle distinguishes Keep on Device from a downloaded file's green
square. Folder badges still inspect a bounded amount of metadata and use unknown
status when they cannot establish a complete answer. Dolphin content previews can
hydrate placeholders; settings explain how to disable them without changing the
user's global preference automatically.

Unresolved engine refusals remain visible even when the upload queue is empty.
Ambiguous online-only files containing local bytes are flagged for review, without a
local-open shortcut; availability jobs covering those paths are refused. A warning
badge distinguishes a modified online-only file from an ordinary cloud placeholder.

The transfer view uses HTTP payload bytes and a measured rolling rate. Retries count
as traffic; sending the last byte is not a confirmed upload. A partial download is
labelled as part of a file. Totals reset with the daemon, and are not billing or
network-interface counters.

Choose folders excludes selected local subtrees from background sync on this device.
Existing files remain in place, and opening an online-only file can still download
it. Excluded items have a grey pause badge. Re-including a subtree requests a fresh
cloud listing, with the normal local-change protections. This is not a cloud-only
folder browser or a command to remove local copies.

Review versions offers Keep both, Use local version and Use cloud version for
primary-drive file conflicts. Incomplete online-only data offers only the cloud
choice. A one-use review expires after ten minutes; a separate confirmation applies
the selected choice. The service pauses background changes while resolving, retains
a read-only Btrfs recovery snapshot outside the sync mount, and checks local identity
and cloud version again. Use local also saves the replaced cloud content and uploads
conditionally; Keep both creates a separate cloud sibling. Concurrent local changes
are preserved. Confirmed cloud operations and their recovery data survive a later
failure, and the result links to the recovery folder.

Resolution requires Btrfs and a state directory on the same filesystem. It refuses
when it cannot secure a snapshot; it never backs up an ambiguous live placeholder
by reading or reflinking it. Recovery snapshots retain the whole tree and can retain
storage until removed. Completed job status is session state; durable recovery files
remain after restart. Shared-library, folder, deleted-remote and missing-identity
conflicts still require separate handling. These local tests do not satisfy the
external tenant/two-device acceptance gate.
