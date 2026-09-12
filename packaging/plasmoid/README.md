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
restarts (`systemctl --user restart plasma-plasmashell.service`). The script
knows which of the two it just did and prints only that one — telling a
first-time installer to restart their shell would be a restart nobody needed,
and a small lie about what was measured.

The installer does not run this script and does not carry the applet, for the
same reason it does not create the subvolume or enroll credentials: it is a
per-user operation on the user's own session data, so the command is printed
instead. The two halves do have to agree on which tray surface a deployment
uses — see below.

## What it talks to

`io.github.franzjeger.OneDriveHydration` on the session bus — the surface
`onedrive-hydration-dbus` serves. It subscribes to `StateChanged` and
`CredentialStateChanged` and never polls daemon state; one cold `GetAll` when the service
(re)appears is the documented complement to the signals, because a freshly
started service does not signal a state it considers unchanged. Eviction —
deliberately absent from the tray menu because it needs a file picker —
lives here: "Free Up Space…" opens the native picker rooted at the sync
folder and calls `Evict` with the path made relative to it. A daemon refusal
comes back as the named `Error.Kept` and is shown with the daemon's reason
verbatim. When the daemon reports a rejected credential, an explicit **Sign in**
button asks the owner-checked `BeginEnrollment` method for a browser/PKCE URL.
Only that active, five-minute interaction polls `EnrollmentStatus`; normal sync
state remains entirely signal-driven. The refresh token is stored directly in
Linux Secret Service before the browser receives a success page.

The wording and precedence in `contents/ui/Presentation.js` mirror `tray.rs`, and
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
`NeedsAttention` — sign-in required — also `NeedsAttention` — unresolved engine issues — pause — active work, queued changes,
synced), plus the flyout. Running `onedrive-hydration-tray` at the same time
shows a second, independent icon; the SNI binary remains the presence for
desktops without plasmashell.

Neither is the right answer everywhere, so the choice is an input rather than
a detection. `onedrive-hydration-install --tray sni|plasmoid|none` records it,
and with nothing said the installer refuses the moment it can see both would
exist — the applet installed for that user *and* the tray unit about to be
enabled. There is deliberately no `auto`: the applet only draws under
plasmashell, the binary draws wherever there is a `StatusNotifierWatcher`, and
which desktop the user logs into is not a fact at install time. The tray unit
is `WantedBy=graphical-session.target` and starts at session start; the
installer runs as root, usually with no session at all; and a machine
installed under Plasma can be logged into under sway tomorrow. Branching on a
running `plasmashell` would make the installed set depend on what happened to
be running when `sudo` was typed.

What the installer *can* measure is the applet package on disk — a durable
fact that survives reboots and desktop switches — so that is what the refusal
reads. `--tray plasmoid` also removes a tray unit an earlier install left, so
answering the refusal actually takes the second icon away instead of only
declining to add one.

The check runs from both sides, because neither side can see everything. The
installer sees the package on disk but not a running unit; `install-plasmoid.sh`
runs as the user inside the session, so it asks `systemctl --user` about
`onedrive-hydration-tray.service` and warns when it is enabled or active. It
reports and never acts: installing an applet is not authority to stop
somebody's service.

## Desktop state and remaining limits

The versioned `DesktopState` JSON exposes the connected mount, account owner/quota,
local disk space, actual pending uploads with errors/retry delay, confirmed upload
history, and unresolved engine issues. It is observational; HydrationAPI owns the work.
`DesktopChanged` and `AvailabilityJobChanged` keep the panel current. Pause gates new
background passes, expires after two hours and resets on restart; on-demand reads still
work. Retry preserves all conditional-write checks. Availability progress counts processed
files and local bytes made available/reclaimed, with cancellation after the current file.

Network byte progress, transfer speed, a complete namespace/download history, initial
storage setup and competing-version resolution remain outside this surface. Account
metadata refreshes every five minutes. Credentials and authorization codes never appear
in these properties. Unresolved availability issues deliberately have no local-open action:
reading such a file can replace ambiguous bytes with a cloud download.

## Activity center and verification

The main view uses a concise status, an indeterminate indicator only during observed
activity, active upload rows with an action to open the containing folder, and a bounded
activity list. Pending changes never become a made-up transfer percentage. Counts and
mount details are behind "Sync details"; long text and lists scroll while the header and
folder action stay fixed. The compact tray button supports Space, Enter and accessibility
press actions. Filenames and errors are rendered as plain text.

Recent activity merges the daemon's bounded persistent upload outcomes with the latest
20 session events (upload starts, sign-in and user availability actions). A cold snapshot
is not replayed as a new event, and a file leaving the active list does not imply success.
Only a confirmed engine outcome becomes an upload-completed entry. Opening a containing
folder may cause Dolphin previews to read files; settings explain the preview preference.

The Qt Quick tests execute the real presentation and view with synthetic state; they do
not contact an account or alter a sync mount. Run on a Plasma 6 development machine:

```sh
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software \
  /usr/lib/qt6/bin/qmltestrunner -input packaging/plasmoid/tests
```

Fedora uses `/usr/lib64/qt6/bin/qmltestrunner`. The `flyout` CI job runs these tests with a
private session bus. Coverage includes active/queued/stopped precedence, genuine busy
indicators, long content in a small popup, keyboard activation, safe folder URLs, and
history that never invents upload success. Rust tests cover the SNI surface independently.

The flyout includes measured payload transfer progress, per-device folder exclusions,
and reviewed file-conflict choices. See the main README for scope and recovery
requirements. Conflict confirmation is a separate action after selecting a version;
incomplete local placeholder bytes can never be uploaded as a complete document.
