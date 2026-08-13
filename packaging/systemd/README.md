# systemd packaging

Release artifacts contain the unprivileged daemon, control CLI, D-Bus state
service, tray and the revision-matched `hydrationd` helper in one payload. The
helper is built directly from the exact HydrationAPI commit pinned by
`Cargo.toml`; its privileged source is not copied or allowed to drift here.

## The installer

Units are not shipped as files; they are generated. A unit must bind one
specific user's mount, numeric uid and runtime socket while keeping credentials
out of the system manager, and a generic unit with shell expansion would weaken
that boundary. `onedrive-hydration-install` (in `crates/installer`) takes the
installation-time facts, validates the machine, and writes concrete units — or
refuses, and says precisely why:

```
onedrive-hydration-install install --user <name> --mount <path> --client-id <uuid>
                                   [--tray sni|plasmoid|none]
onedrive-hydration-install uninstall --user <name> --mount <path> [--and-unmount]
onedrive-hydration-install render  --user <name> --mount <path> --client-id <uuid>
                                   [--tray sni|plasmoid|none]
```

It refuses to write anything when: the kernel predates fanotify pre-content
events (Linux 6.14; measured with a real `FAN_PRE_ACCESS` mark when run as
root, inferred from the version otherwise); the sync root is not its own mount,
or is on a filesystem without `SB_I_ALLOW_HSM` (exactly ext4, btrfs and xfs —
tmpfs is called out by name); another mount exposes the same files (DESIGN.md
§6.4a — detected, never preventable); the fstab entry for the sync root is not
`noauto`; Secret Service is neither owned nor activatable on the user's session
bus (enrollment fails closed without it); a payload binary the units point at
is missing; the named user does not resolve; the installer itself is running in
a private mount namespace (its answers would describe the sandbox, not the
machine); an existing generated file — a unit or the D-Bus activation file —
differs from what would be generated (`--force` to overwrite); two tray
surfaces would end up installed and no `--tray` said which one this deployment
uses; or — checked in the generated text, not assumed from the templates — a
helper unit carries any namespace-creating directive.

Every one of those refusals is exercised in `crates/installer/tests/refusals.rs`,
including the namespace scan against a deliberately poisoned template. A
refusal that has never been seen to fire is not a check.

What the installer never does, by design: it does not create or delete the
btrfs subvolume (the command is printed instead — that is your storage layout
and a destructive operation); it does not touch `/etc/fstab` without
`--consent-fstab`, and there is no code path that composes an entry without
`noauto`; it does not enroll credentials or invent a client id — `--client-id`
is required, and it is public configuration, never a secret; and it does not
install or remove the Plasma applet or the Dolphin action, which are per-user
operations on the user's own session data — `packaging/plasmoid/install-plasmoid.sh`
and `packaging/dolphin/install-servicemenu.sh` are printed instead, and
`uninstall` names the `kpackagetool6 --remove` line rather than running it.

`--prefix <dir>` rehearses an install or uninstall against a scratch root:
files land under the prefix, no command is ever executed. `render` prints the
units for review, and for diffing a deployed set against what the current
version generates.

## The user half: three surfaces, three different triggers

The sync daemon, the D-Bus state service and the tray are one product but
three lifecycles, and the units say so instead of sharing one `WantedBy=`:

- **`onedrive-hydration.service`** starts with `default.target` — it must run
  in any session that can reach the mount, graphical or not. It is
  deliberately *not* ordered after the credential store, because that cannot
  be expressed: on the verified deployment `org.freedesktop.secrets` is owned
  by `ksecretd`, which PAM starts inside the login session's scope
  (`UserUnit=n/a`, so there is no user unit to name in `After=`), and the name
  is not activatable, so the bus cannot summon it either. Measured at login
  2026-08-12: this unit started with `default.target` at t=20.7s and failed;
  the store appeared at t=25.8s; the `Restart=on-failure` retry at t=25.7s
  succeeded — recovery by luck, logged as `PermissionDenied … could not read
  the OneDrive credential`, indistinguishable from a lost credential. An
  `After=` against a job outside the unit's own start transaction orders
  nothing, so the *daemon* closes the gap: at startup it waits — bounded,
  sixty seconds — for the name to be owned or activatable, and its messages
  distinguish "the store is not up yet" (an outage; do not re-enroll) from
  "there is no credential" (sign in).

- **`onedrive-hydration-dbus.service`** is enabled nowhere: it is
  D-Bus-activated. The installer writes an activation file to
  `~/.local/share/dbus-1/services/` whose `SystemdService=` names the unit, so
  the first thing that talks to `io.github.franzjeger.OneDriveHydration` — the
  tray's initial cold property read, a `busctl introspect`, the future
  flyout — starts it, and a session with no subscriber runs no state service
  at all. Activation is what the bus is for: the activating call is queued
  until the name is up, so the first caller sees a slow reply, never a missing
  one. `Type=dbus` ties readiness to name acquisition, and `Restart=on-failure`
  covers crashes only — a stopped sync daemon is a *reported state*
  (`DaemonRunning=false`), not a failure of this service.

- **`onedrive-hydration-tray.service`** needs a graphical session and nothing
  else: `WantedBy=graphical-session.target`, with `After=` and `PartOf=` the
  same, so it starts when a session exists and stops with it. Started earlier
  it would find no `org.kde.StatusNotifierWatcher` and exit, by its own
  design — correct behaviour at the wrong moment. Even at the right moment the
  watcher can lag (on the verified desktop it belongs to kded6, started around
  the same target), so the unit retries on a five-second spacing and gives up
  after ~ten attempts: a desktop that will never show a tray gets one visibly
  failed unit, not an all-session respawn loop.

  It is also the one unit a deployment may not want, because it is not the
  only tray surface. The Plasma applet in `packaging/plasmoid/` is a
  system-tray entry in its own right — a running plasmashell adopts and
  instantiates it within seconds of `kpackagetool6` finishing, no restart —
  so a Plasma user who runs both this installer and `install-plasmoid.sh`
  gets two identical icons. `--tray` says which surface this deployment uses:
  `sni` writes and enables this unit, `plasmoid` writes neither it nor its
  enablement link (and stops and removes one an earlier install left, so the
  answer removes the second icon rather than only declining to add another),
  `none` installs no tray at all and says what that costs — the §6.4a
  exposure warning then exists only in `onedrive-hydrationctl status`.

  Left unsaid it defaults to `sni`, *unless* the applet is already installed
  for that user, which is refused until one of the three is named. There is
  no `auto`. The applet only draws under plasmashell and the binary draws
  wherever a `StatusNotifierWatcher` exists, so the answer depends on the
  desktop — and the desktop is not a fact at install time: this unit starts at
  *session* start, the installer runs as root from a shell that usually has no
  session, and a machine installed under Plasma can be logged into under sway
  tomorrow. Detecting a running `plasmashell` would make the installed set
  depend on what happened to be running when `sudo` was typed, which is the
  same shape as every other refusal here — a deployment whose facts came from
  somewhere other than the deployment. What *is* durable, and what the check
  reads instead, is whether the applet package is on disk
  (`~/.local/share/plasma/plasmoids/`, or the system tree a distribution
  package would use). The binary check follows the decision: with the applet
  as the surface, no unit points at `onedrive-hydration-tray`, so its absence
  is no longer a refusal.

Uninstall makes one promise that outranks tidiness: `hydrationd` is never left
stopped — or deleted — while the sync root is still mounted, because a marked
mount with nobody answering, or a fresh unmarked one, is the fail-open state
this project exists to prevent. While the mount is up it refuses unless told
`--and-unmount`, and then the order is: stop `hydrationd.path` (so nothing
restarts the helper mid-removal), `umount` (whose stop job reaches the helper
through `RequiresMountsFor=`), verify the mount is gone, and only then remove
files.

## Safety properties the units must retain

The generated units follow the hand-written set that was verified on a real
deployment across a real reboot, not HydrationAPI's example units. The
properties that are measurements, not preferences:

- a separate real mount for the sync root, `noauto` in fstab — the mount must
  never exist before the helper that marks it;
- `RequiresMountsFor=` rather than `BindsTo=` — with `BindsTo=` systemd reads
  the helper's own `umount2(MNT_DETACH)` on the way out as a deliberate stop
  and suppresses the restart, permanently;
- `hydrationd.path` triggering `hydrationd.service` directly on
  `PathExists=`, with `StartLimitIntervalSec=0` on the service and no
  `[Install]` section on it — the path unit is the only trigger;
- an explicit `--peer-uid`, and only `CAP_SYS_ADMIN` + `CAP_DAC_OVERRIDE` on
  the helper (`CAP_DAC_OVERRIDE` because a 0700 home defeats `CAP_SYS_ADMIN`
  alone — measured);
- **no mount namespace on the helper, ever.** `fanotify_mark(FAN_MARK_MOUNT)`
  marks the vfsmount in the caller's namespace; a helper in a namespace of its
  own marks a private copy, the unit reports active, and every read from the
  user's session returns the zeros a placeholder is made of. Measured, each of
  `PrivateTmp=`, `PrivateNetwork=`, `ProtectKernelTunables=`,
  `ProtectControlGroups=` and `ProtectKernelModules=` *alone* creates that
  namespace (verified by comparing `/proc/self/ns/mnt` against the host; it
  happened twice on a real deployment). An earlier revision of this file
  listed `PrivateNetwork=yes` among the *required* properties — that was
  measured wrong and dangerous. The network denial is
  `RestrictAddressFamilies=AF_UNIX`, which is the same guarantee for a process
  that only ever speaks to a unix socket and costs no namespace. HydrationAPI's
  `deploy/hydrationd.service` and `crates/hydrationd/src/selfcheck.rs` both
  record this; the installer refuses to write a helper unit carrying any of
  the five, and `hydrationd` refuses to start inside a private namespace, so
  the mistake has to get past both to be silent.

The templates live in `crates/installer/templates/`; their comments carry the
rest of the reasoning and are kept in the generated files on purpose.
