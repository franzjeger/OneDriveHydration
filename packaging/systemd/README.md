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
onedrive-hydration-install uninstall --user <name> --mount <path> [--and-unmount]
onedrive-hydration-install render  --user <name> --mount <path> --client-id <uuid>
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
machine); an existing unit differs from what would be generated (`--force` to
overwrite); or — checked in the generated text, not assumed from the templates —
a helper unit carries any namespace-creating directive.

Every one of those refusals is exercised in `crates/installer/tests/refusals.rs`,
including the namespace scan against a deliberately poisoned template. A
refusal that has never been seen to fire is not a check.

What the installer never does, by design: it does not create or delete the
btrfs subvolume (the command is printed instead — that is your storage layout
and a destructive operation); it does not touch `/etc/fstab` without
`--consent-fstab`, and there is no code path that composes an entry without
`noauto`; it does not enroll credentials or invent a client id — `--client-id`
is required, and it is public configuration, never a secret.

`--prefix <dir>` rehearses an install or uninstall against a scratch root:
files land under the prefix, no command is ever executed. `render` prints the
units for review, and for diffing a deployed set against what the current
version generates.

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
