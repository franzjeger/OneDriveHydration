# The flyout plasmoid

The panel a person sees when they click the tray entry, for Plasma 6. It is
shipped as data — a QML applet plasmashell loads — rather than linked as a
dependency, for the same reason the tray binary is a StatusNotifierItem over
zbus: on this desktop the panel is already a QML host and the tray is already
D-Bus, so a toolkit would buy nothing but a second look-and-feel. Zero new
Rust dependencies; `cargo deny check` sees an unchanged graph.

Install per user with `./install-plasmoid.sh`. The icons are a prerequisite:
run `../icons/install-icons.sh` first or the panel renders a generic
fallback. A first install is adopted by the running system tray by itself;
after an upgrade the already-loaded QML keeps running until plasmashell
restarts (`systemctl --user restart plasma-plasmashell.service`).

## What it talks to

`io.github.franzjeger.OneDriveHydration` on the session bus — the surface
`onedrive-hydration-dbus` serves. It subscribes to `StateChanged` and
`CredentialStateChanged` and never polls; one cold `GetAll` when the service
(re)appears is the documented complement to the signals, because a freshly
started service does not signal a state it considers unchanged. Eviction —
deliberately absent from the tray menu because it needs a file picker —
lives here: "Free Up Space…" opens the native picker rooted at the sync
folder and calls `Evict` with the path made relative to it. A daemon refusal
comes back as the named `Error.Kept` and is shown with the daemon's reason
verbatim.

The wording of every state is copied from `tray.rs` verbatim, and
`crates/onedrive-daemon/tests/plasmoid_package.rs` pins the two against each
other (deriving the expected strings from `present()` where they are static),
along with the bus names and icon names. Those tests are drift alarms, not
behaviour tests: QML does not run under cargo. Behaviour was verified against
a live bus — see below.

## Measured on Plasma 6.7.4, not taken from documentation

* `org.kde.plasma.workspace.dbus` decodes D-Bus `t` (u64) values into
  wrapper value types: `Excluded` arrives as `{value: 167652}`, not a
  number. Every numeric read goes through `u64()` in main.qml.
* A bus signal reaches QML as a call to a function named `dbus<Member>` on a
  `SignalWatcher` — there is no signal to attach a handler to. The
  subscription survives a service restart without re-arming.
* `SessionBus.asyncCall(message, resolve, reject)` passes the *pending
  reply* to both callbacks; a rejection carries its error at `reply.error`,
  not as the argument itself.
* A running plasmashell adopts a freshly *installed* NotificationArea applet
  immediately (appended to the system tray's `knownItems`/`extraItems` and
  instantiated within seconds, no restart). Upgrades keep executing the old
  QML until the shell restarts.
* `X-Plasma-DBusActivationService`, which older documentation offers for
  loading an applet only while a service runs, does not exist in this
  plasma-workspace (no such string in any of its binaries). Just as well: the
  flyout must stay in the tray while the service is *down* to say so — its
  most important state is the one auto-unloading would hide.

## Relationship to the tray binary

On Plasma, this plasmoid *is* the tray presence: same icons, same states,
same precedence (service absent, daemon stopped, exposures — rendered
`NeedsAttention` — sign-in required — also `NeedsAttention` — unsent,
synced), plus the flyout. Running `onedrive-hydration-tray` at the same time
shows a second, independent icon; the SNI binary remains the presence for
desktops without plasmashell.

## What the D-Bus surface cannot answer yet

Built deliberately against what exists rather than inventing data:

* The mount path is not on the bus, so the flyout is told through plasmoid
  configuration (defaulting to `~/OneDrive`) the way the tray is told
  through `--mount`.
* Placeholder and unsent figures are file counts; there are no byte totals,
  so "how much disk would hydrating cost" cannot be shown.
* No account identity, no quota, no per-file transfer progress, no recent
  activity, no conflict list.
* Credential health is now on the surface (`CredentialState`, with
  `CredentialStateChanged`) and shown — "Sign-in required" names
  `tools/pkce-enroll.py`, the enrollment that works on this deployment,
  because Conditional Access blocks the daemon's device-code flow. What the
  flyout still cannot do is *start* enrollment: an in-product sign-in flow
  waits on the PKCE threat-model review (docs/ROADMAP.md M1), and the flyout
  does not even know the daemon's client id. There is deliberately no
  sign-in button — a button that cannot do the thing it names is worse than
  a sentence that can be followed.

Widening the surface is its own task with its own measurements; this flyout
shows everything the surface can currently say and nothing it cannot.
