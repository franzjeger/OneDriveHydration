# Groundwork: Download visibility — the placeholder emblem and the plasmoid's downloading field

Design only. No emblem plugin and no download counter exist in either repo. Grepped:
the only `KOverlayIconPlugin`/`KVersionControlPlugin` mention in this product is prose
in `docs/DOLPHIN-GROUNDWORK.md` and the open roadmap item (`docs/ROADMAP.md:63-69`); the
only in-flight counter anywhere in `hydration-client` is the *upload*-side `in_flight`,
never a download gauge (the watch line carries `unsent`/`excluded`/`exposures` only —
`daemon_loop.rs:339-344`). This document is what that code will be written from; the test
lists in §6 are meant to be written *first* and to fail against the current trees.

It follows the house style of `docs/DOLPHIN-GROUNDWORK.md` (this repo) and
`HydrationAPI/docs/KEEP-ON-DEVICE-GROUNDWORK.md` / `AUTO-EVICTION-GROUNDWORK.md`: every
claim is cited to `file:line` (or a URL for an external API) because the framework's law
is measured-not-recalled, the obvious-but-wrong alternative is named at each fork, and the
critique at the end is kept including what it says about this document's own weaknesses.

**Home of this file.** `OneDriveHydration/docs/` is the right home — it is where
`DOLPHIN-GROUNDWORK.md` and `ROADMAP.md` already live, and both surfaces are product shell,
not framework. The one framework-side change (§3.1) is flagged as owned by HydrationAPI and
lands there; everything else is this repo. The `HydrationAPI/docs/` fallback was not needed.

---

## The two features, and why they are the missing "cloud client" signals

A Windows OneDrive user reads two things at a glance that this product cannot yet show:
*which files are on the device* (the green check vs the blue cloud, per file, in the file
manager) and *whether the client is pulling something down right now* (the tray's activity
line). The framework already knows both facts — a placeholder is marked on disk, and the
client is the process that serves every byte — but neither is surfaced to a person. These
two features close exactly that gap, and they turn out to share one new signal (§3, §5).

- **Feature 1 (emblems), one sentence:** a Dolphin overlay plugin draws a per-file badge —
  cloud-only vs on-device, and later a downloading badge — by reading the `dehydrated`
  xattr the framework already writes, for only the files the user is currently looking at,
  never opening a byte.
- **Feature 2 (download field), one sentence:** the client publishes a count of hydrations
  in flight on the `watch` line it already broadcasts, which the D-Bus bridge republishes as
  a new property + signal and the plasmoid renders as one "Downloading N" row beside the two
  counters it already shows.

The recommended build order (§5) ships Feature 1's two stable states first with *zero*
framework change, then Feature 2, then the live-refresh channel both richer states need.

---

## What this establishes before any code is written

Five things the five-investigator pass pinned down, each of which shapes the feature:

- **Residency is one xattr, and reading it is metadata, not content.** A cloud-only
  placeholder carries `user.hydration.dehydrated`; a resident file does not
  (`hydration-protocol/src/lib.rs:168`, "This — not `st_blocks` — is what 'is a placeholder'
  means", `:162-168`). The framework's own placeholder test is exactly one presence-probe
  `getxattr` — `placeholder::has_mark` (`placeholder.rs:421-433`), size arg `0`, NULL buffer,
  `ENODATA`/`ENOTSUP` ⇒ absent ⇒ resident. This is the emblem's whole input, and it is the
  same predicate the framework trusts. **Never `st_blocks`** — a 499 MB placeholder reported
  `blocks=0`, and an empty ext4 placeholder is charged a block for its xattrs, so the count
  cannot separate the cases; this was a production bug (`placeholder.rs:345-419`;
  `DOLPHIN-GROUNDWORK.md:103-108`).

- **The "downloading" third state has no on-disk marker, deliberately.** There is no
  `user.hydration.building` on the target file: an "under construction" mark was *removed*
  because every `user.*` xattr is forgeable by any same-uid process, and a forged one made a
  real placeholder serve zeros (`lib.rs:190-198`, "There is deliberately no 'under
  construction' mark here"). A partially-filled placeholder keeps the `dehydrated` mark until
  the whole object is present (`settle_range` keeps it; `finish_hydration` clears it — see
  `DESIGN.md:1345`). So the transient DOWNLOADING state can *only* come from the live client,
  never from the filesystem — which is exactly the signal Feature 2 already needs (§3, §5).

- **The Dolphin overlay interface is per-visible-item, not per-tree.** `KOverlayIconPlugin::
  getOverlays(const QUrl &)` is called for the items Dolphin is currently drawing, on the
  main thread, and "must not block … have a cache … return an empty list and call
  overlaysChanged when the information is available" (KIO `koverlayiconplugin.h`, doc at
  <https://api.kde.org/koverlayiconplugin.html>). This is structurally O(files the user is
  looking at) — tens to hundreds — never the 166k-placeholder tree, which is the performance
  law the emblem must obey (§4).

- **There is exactly one download vantage, and it is single and sequential.** Every byte the
  user pulls flows through the client's `Daemon::serve`, one `FromHelper::Fetch` at a time
  (`hydration-client/src/lib.rs:196-274`; `provider.fetch` at `:228-234`), and the daemon
  serves "one helper at a time" (`daemon_loop.rs:1527-1531`). So "downloading" is a count of
  0 or 1 at the client today — an aggregate, not a list — and a per-file progress bar is a
  category error, because a fetch is a demanded *span* (a header read of a multi-GB object is
  a 4 KiB fetch), not a whole file (§3.3).

- **The status wire is append-only and push-based, end to end.** The client's `watch` line
  is `WatchState::line()` — `"unsent={} excluded={} exposures={}"` — with "new keys may only
  ever be appended" and readers told to ignore unknown keys (`daemon_loop.rs:333-344`),
  broadcast once a second, deduped (`:1521-1522`). The product bridge honors that: unknown
  keys are skipped, not refused (`dbus.rs:96-114`). So a `downloading=N` key is
  forward-compatible with every existing subscriber, and the plasmoid already subscribes by
  signal and never polls (`main.qml:13-16`).

Consequence: Feature 1's two stable states need no framework or IPC change at all; the third
state and Feature 2 share one new signal — a client-side in-flight fetch count — surfaced two
ways (a watch-line key for the aggregate, a socket push for the per-path emblem).

---

## 1. Feature 1 — the emblem mechanism

### 1.1 The chosen plugin: `KOverlayIconPlugin` (KF6, install namespace `kf6/overlayicon`)

**Decision: a `KOverlayIconPlugin` — a compiled Qt6/KF6 C++ `.so` — the same mechanism
Nextcloud's current Dolphin integration uses, installed into the KF6 plugin dir under
`kf6/overlayicon`, linking `KF6::KIOCore`.** The class is `KOverlayIconPlugin` with the pure
virtual `QStringList getOverlays(const QUrl &item)` and the signal `void overlaysChanged(const
QUrl &url, const QStringList &overlays)` (KIO `src/core/koverlayiconplugin.h`; doc
<https://api.kde.org/koverlayiconplugin.html>). Dolphin wires `overlaysChanged` into
`KFileItemModelRolesUpdater` and re-queries only the affected item
(<https://github.com/KDE/dolphin/blob/master/src/kitemviews/kfileitemmodelrolesupdater.cpp>).
The reference is Nextcloud's `ownclouddolphinoverlayplugin.cpp`
(<https://github.com/nextcloud/desktop/tree/master/shell_integration/dolphin>).

**Rejected: `KVersionControlPlugin`** (the older git/svn route, used by the legacy dschmidt
ownCloud plugin and Insync). Three reasons, each decisive on its own:
- It only activates for a tree that *physically contains a sentinel entry* — Dolphin checks
  `QFile::exists(directory + '/' + plugin->fileName())` before lighting the plugin up
  (`KDE/dolphin src/views/versioncontrol/versioncontrolobserver.cpp`), so a 166k-file
  OneDrive root would need a planted marker file, and only **one** VCS plugin can own a tree
  — colliding with any user who keeps a real git repo inside OneDrive.
- It renders in one corner only; `getOverlays` returns a list drawn at multiple corners.
- Nextcloud itself migrated *away* from it to `KOverlayIconPlugin`. Following the maintained
  route is following the one with a live reference implementation.

**Rejected: any shell / `.desktop` / QML route.** There is no data-only path to per-file
emblems — `DOLPHIN-GROUNDWORK.md:14-31,24-27` establishes this, measured: the servicemenu
mechanism this repo already ships (Free Up Space / the future Keep on Device) is
`KFileItemAction`, i.e. context-menu actions, not overlays. Feature 1 therefore introduces
the first compiled C++/CMake/Qt6/KF6 artifact to a repo that ships shell + a QML plasmoid.
**That build/dependency escalation is the real gate, and it is the same one the roadmap
already frames** (`ROADMAP.md:63-69`); the API choice above is the easy half.

### 1.2 How it discovers per-file status — one `lgetxattr`, proven event-free

**Decision: for each URL Dolphin asks about, the plugin's answer is one presence-probe
`lgetxattr(path, "user.hydration.dehydrated", NULL, 0)`, the exact call `placeholder::has_mark`
makes (`placeholder.rs:421-433`):**

```
lgetxattr(path, "user.hydration.dehydrated", NULL, 0)
  rc >= 0                    -> CLOUD-ONLY placeholder   (cloud emblem)
  errno == ENODATA/ENOATTR   -> RESIDENT / On Device      (check emblem)
  errno == ENOTSUP           -> RESIDENT (fs has no xattrs; no placeholders possible)
  errno == ENOENT            -> file gone; no emblem
```

`lgetxattr`, not `getxattr`, so a symlink is not followed into a content read of its target;
size arg `0` with a NULL buffer, so no value is fetched and no buffer is allocated — presence
is the whole test.

**This is event-free — it fires no `FAN_PRE_ACCESS`, cannot hydrate, and cannot deadlock
under §6a-ter — and the framework relies on the same fact.** The hook is
`fsnotify_file_area_perm()` on `MAY_READ | MAY_WRITE | MAY_ACCESS`, plus `fsnotify_mmap_perm()`
and `fsnotify_truncate_perm()` (`DESIGN.md:157-162`); `getxattr`/`lgetxattr`/`listxattr`
traverse none of these — they are pure metadata. Stated directly: "`FAN_PRE_ACCESS` fires on
content access, not on `stat(2)` … `find`, `ls`, `du`, `tree` … are free … only tools that
actually read bytes are in danger" (`DESIGN.md:1162`). And §6a-ter is scoped explicitly:
setting/reading a `user.*` xattr "fires no pre-content event … §6a-ter is about content, not
metadata" (`DESIGN.md:1335`). Corroborated stronger-than-needed by measurement: even
`lseek(SEEK_DATA)` fires zero events (`probes/seekdata.c`), and `getxattr` is lighter still —
it reads no bytes at all. **The one gap: no probe measures `getxattr` *specifically* under a
live mark, so §6 makes it P1, the gate — repo law is to measure before a test asserts.**

**Rejected: `open()` + `read()` / `SEEK_DATA` / `st_blocks` to determine status.** `open()`
is harmless but pointless; `read()` *hydrates* — an emblem scan that read content would
hydrate every one of 166k placeholders, or deadlock; `SEEK_DATA` answers a different question
(does it hold bytes) and is unnecessary when the mark is the truth; `st_blocks` is the
production bug above. `lgetxattr` alone is the whole test.

### 1.3 The state → emblem mapping

`getOverlays` returns arbitrary themed icon *names*, so the mapping is our choice:

| State | Source of truth | Emblem (name returned) |
|---|---|---|
| CLOUD-ONLY placeholder | `dehydrated` present | `onedrive-cloud` — the blue-cloud analog; Breeze `vcs-update-required` as the theme fallback |
| RESIDENT / On Device | `dehydrated` absent | a green check: Breeze built-in `vcs-normal`, or a branded `onedrive-synced` if we want parity art |
| DOWNLOADING | client in-flight set (not the fs) | `onedrive-partial` — Slice 4, socket-only (§5) |

The three custom names `onedrive-cloud` / `onedrive-partial` / `onedrive-upload` are **already
reserved** for exactly this milestone — 16 px file-manager overlay emblems the donor client
kept, held in the donor repo and "deliberately not shipped here until that work names them"
(`packaging/icons/README.md:46-49`). Two honesties: (a) the donor set has *no* resident
badge, so the green check is either the Breeze built-in `vcs-normal` (ships today, exists in
Breeze) or one new SVG — one line either way since `getOverlays` takes any icon name; (b) the
mapping intentionally leaves `onedrive-upload` for a later upload-in-flight emblem, out of
scope for these two features but reserved so the vocabulary does not drift.

Windows draws a check on *every* on-device file; this table matches that. If the check reads
as noise on a resident-heavy folder, dropping RESIDENT to "no overlay" (absence of a cloud =
on device) is a one-line change and a measurable UX question, not a redesign.

### 1.4 The build/packaging shape

A new `packaging/dolphin/overlay/` subdir beside the existing servicemenu wrappers: a
`CMakeLists.txt` using ExtraCMakeModules + `kcoreaddons_add_plugin`, one C++ class
(`Q_OBJECT` + `Q_PLUGIN_METADATA(IID "org.kde.overlayicon.…")`), and a `.json` metadata file
— installed as a versioned `.so` into the KF6 plugin dir, linking `KF6::KIOCore`, mirroring
Nextcloud's `CMakeLists.txt`. This is a **root-scope install** (`/usr/lib/...`), unlike the
per-user servicemenu and icons this product ships today, and CI gains a second toolchain
(`DOLPHIN-GROUNDWORK.md:14-22`). **It must not collide with the stale, package-less,
root-owned `/usr/lib/qt6/plugins/kf6/overlayicon/onedrive-overlay.so`** left by the donor
client, which reads `user.onedrive.syncstate` (0/400 files carry it — it draws nothing but
still loads): two overlay sources for the same files is "the same shape as the tray/plasmoid
collision that `--tray` now refuses" (`DOLPHIN-GROUNDWORK.md:121-142`). The installer must
report it and print the `sudo rm`, exactly as `install-servicemenu.sh` already does for it.

### 1.5 The refresh strategy — relist-first, push-later

An xattr change alone does *not* make Dolphin re-poll: `setxattr` moves ctime, not mtime, and
Dolphin does not watch xattrs. Dolphin re-queries overlays only on (a) a directory relist
(KDirLister/KDirWatch dir-level change, or F5) or (b) the plugin's own `overlaysChanged(QUrl)`.

- **Slice 1 (ships first): relist-only refresh, from a `QHash` cache.** `getOverlays` answers
  from an in-process cache; on a miss it enqueues the path to a short-lived worker that does
  the one `lgetxattr` and emits `overlaysChanged` — strictly honoring the "must not block the
  main thread" rule (`koverlayiconplugin.h`). A hydrate rewrites content (size/mtime move) and
  an eviction swaps the inode, both of which a relist re-queries; whether that is prompt enough
  is P3 (§6), and it should be *measured* before being called inadequate
  (`DOLPHIN-GROUNDWORK.md:112-115`).
- **Slice 3: live push, the Nextcloud model.** A per-path status socket: `getOverlays`
  subscribes the path, and when a file flips resident↔cloud-only the client pushes
  `STATUS:<state>:<path>`, the plugin updates its cache and emits `overlaysChanged(QUrl)` so
  Dolphin refreshes that one item with no relist (Nextcloud's `RETRIEVE_FILE_STATUS` /
  `STATUS` / `UPDATE_VIEW` line protocol over `$XDG_RUNTIME_DIR/<app>/socket`). This is the
  same channel the DOWNLOADING emblem needs, so it lands once and serves both.

**Rejected: socket-first (a per-path endpoint before shipping any emblem).** It is the
architecturally "clean" route and Nextcloud takes it — but it is net-new IPC on both the
client and the plugin before a single badge appears, when the two stable states need *nothing*
new. Ship the value first; add the socket exactly when the third state forces it.

---

## 2. Feature 2 — the plasmoid download field

### 2.1 The exact field

**Decision: one aggregate count, "Downloading N file(s)", as a new row in the flyout's
counters grid, visible only while a fetch is in flight.** It sits beside the two rows that
exist today — "Waiting to upload: N changes" and "Cloud-only placeholders: M files"
(`FullRepresentation.qml:82-96`) — and is rendered from a new `downloading` mirror property
on the plasmoid root, decoded through the existing `u64()` and updated by the existing
signal machinery.

Not a percentage, not a bar, not a per-file list — see §3.3. A spinner + count is the honest
shape for a serialized, span-scoped fetch.

**Rejected: a per-file download list / a whole-file progress bar (v1).** Two measured
reasons: the client serves one fetch at a time (`daemon_loop.rs:1527-1531`,
`lib.rs:196-274`), so a list carries ≤1 entry and buys nothing over a count; and a fetch is a
demanded *span*, not a whole file, so "63% of file.iso" is dishonest for on-demand reads and
only meaningful for a whole-object Keep-on-Device pull (§3.3). Defer both to a later slice
that also needs the Slice-3 push channel.

### 2.2 Aggregate-first, and why the two features share one signal

The DOWNLOADING emblem (§1.3) and this field are the *same underlying fact* — the client's
set of in-flight fetches — surfaced at two granularities. Feature 2 needs only the aggregate
count, which rides the existing once-a-second `watch` broadcast (`daemon_loop.rs:1521`).
Feature 1's per-file badge needs the *paths*, which needs the Slice-3 push socket. So the
phased order (§5) introduces the count first (cheap, on the channel that exists) and the
per-path push later (only when the emblem's third state forces it), never building two
parallel download-tracking mechanisms.

---

## 3. The signal that must be added, and the path it travels

### 3.1 Framework (HydrationAPI) — a client-side in-flight gauge (net-new; flag to the framework)

The count does not exist. **Add an `Arc<AtomicU64>` "fetches in flight" to the client, bumped
around the one `provider.fetch` call in `Daemon::serve` and sampled into the watch line.**

- **Where to bump:** `hydration-client/src/lib.rs:196-274`. The gauge must be raised on the
  begin/fetch path (`:227-234`) and lowered on **every** exit — `finish` (`:239`), `abort`
  (`:244`), and the `?` early-return inside `conn.begin(req.id, span.len)?` (`:227`). An RAII
  guard (increment on construction just before `begin`, decrement on `Drop`) is the safe form;
  a hand-written inc/dec leaks a stuck "downloading" the first time `begin` errors. The
  `span.end() > size` refusal (`:212-223`) `continue`s *before* `begin`, so it never
  incremented — correct, leave it alone.
- **Where to sample:** append `downloading: u64` to `WatchState` (`daemon_loop.rs:322-331`),
  append ` downloading={}` to `WatchState::line()` (`:339-344`, keeping the existing key order
  per the append-only rule at `:336-338`), and read the atomic in `watch_state()`
  (`:354-366`). Thread the `Arc<AtomicU64>` into `Daemon::new` (constructed per connection at
  `daemon_loop.rs:1538`) and clone it into the broadcast thread's `watch_state` call.
- **Why the client, not the worker.** The privileged helper also "knows" a fetch is happening
  (via `Step::Progress`) but surfaces it only as a journal `eprintln`, and it lacks the object
  size/path the client has first-hand; routing this through a new `FromHelper` progress message
  would add a root→client message for data the client already owns and fights §6b's "the
  privileged side knows as little as possible." The client is the correct and only vantage.

This is the sole framework edit and is owned by HydrationAPI. Until it lands, the bridge and
plasmoid below simply never see the key and render nothing — graceful degradation is the whole
point of the append-only contract.

### 3.2 Product (OneDriveHydration) — bridge and D-Bus surface

The chain is `watch` line → `apply_state_line` → D-Bus property + signal → plasmoid. Mirror
the **credential** precedent exactly — a separate property and a separate signal, never a new
argument on the pinned one:

- **`dbus.rs`:** add `downloading: u64` to `DaemonState` (`:76-85`); add `"downloading" =>
  state.downloading = value,` to `apply_state_line`'s match (`:105-110`); add a `Downloading`
  property (`-> u64`, beside `exposures()` at `:448-451`); add a **new** signal
  `DownloadChanged(t)` (`#[zbus(signal)]`, beside `credential_changed` at `:531-532`); and emit
  both from `publish_state` (`:562-592`) — a `downloading_changed` PropertiesChanged when it
  moved, then `DownloadChanged`.
- **The pinned contract:** `StateChanged`'s signature is `(bool,u64,u64,u64)` and subscribers
  "silently drop anything shaped differently … additive only for *new members*, never for new
  arguments on old ones" (`dbus.rs:22-28`; the tray/plasmoid decode it by exact shape). So the
  count must **not** become a fifth argument on `StateChanged`. The credential state solved this
  identically — a separate `credential_state` property + `CredentialStateChanged` signal +
  `publish_credential` (`dbus.rs:464-467,524-532,546-560`) — and that is the template to copy.

### 3.3 Why the field is a count, not a bar — the measured category error

A fetch is a demanded **span**, not a whole file. `Daemon::serve` builds `span =
Span::new(req.offset, req.len)` from the helper's request and holds `Body` to exactly that
length (`lib.rs:201,224-227`); the helper's readahead window is bounded, so a header read of a
2.77 GiB object is a small-span fetch, not a whole-file download. `body.promised`/`span.len` is
therefore *not* the object size, and "X of Y bytes toward the file" is a number the framework
does not have per fetch. Whole-file progress is honest only for a whole-object pull
(Keep-on-Device), which is a later slice. For v1, "N downloading" is the honest signal.

### 3.4 The 1 Hz sampling caveat

The count rides the once-a-second, deduped broadcast (`daemon_loop.rs:1521-1522`), which
deliberately refuses per-change notification. A header-read fetch can open and close inside one
second and never be sampled — the field will miss or flicker on very short hydrations. For a
"sustained download in progress" indicator that is acceptable and even desirable (it will not
strobe on every 4 KiB metadata read); for reliable per-fetch visibility it is not, and fixing
it means pushing a broadcast on fetch start/finish, which contradicts the stated 1 Hz design
and needs sign-off. P5 (§6) measures real fetch durations before we promise more than "sustained
download."

---

## 4. Safety — the three properties this UI must never violate

### 4.1 The emblem UI never reads content (the §6a-ter guarantee)

§6a-ter is: a read (or write) inside the marked sync mount, performed by a process that could
be asked to answer the very event it triggers, is a deadlock — and it has "appeared in eight
distinct disguises" (`CLAUDE.md`). The emblem plugin is safe by construction because it *only*
ever issues `lgetxattr`, which is metadata and fires no pre-content event (§1.2;
`DESIGN.md:157-162,1162,1335`). It never calls `open`/`read`/`mmap`/`truncate` on a sync-root
file. This must remain true for every code path: `getOverlays` receives a `QUrl` and returns
icon *names* — never a file path that could re-enter the mount as a read — and the custom icons
we return are theme names, not files under the sync root
(verified for the render path in `kfileitemmodelrolesupdater.cpp`; the plugin never opens the
file). Feature 2 reads no file content and no xattr at all — it counts bytes the client is
already serving — so §6a-ter is not implicated by it whatsoever.

**The neighbour hazard to flag, not owned here:** a *thumbnailer* running over the same folder
is a real hydration hazard — a thumbnailer that reads an image/PDF placeholder *will* hydrate
it via `FAN_PRE_ACCESS`. The emblem plugin is safe; the file manager's thumbnail generation
over cloud-only files is a separate risk to Feature 1's neighbours and belongs in the roadmap
note, not in this plugin.

### 4.2 The performance bound: O(files the user is looking at)

`getOverlays` is called for the items Dolphin is *drawing* (`koverlayiconplugin.h`;
`kfileitemmodelrolesupdater.cpp`), never for the tree. At 166k placeholders this is the
difference between a working feature and a full-tree walk on every folder open. The bound is
structural to the API, and the `QHash` cache + async-miss pattern keeps even the visible-set
cost off the main thread. P4 (§6) measures the one `lgetxattr` latency across a large directory
to confirm the cache alone suffices.

### 4.3 The mark is unforgeable in the dangerous direction

A same-uid process can forge `user.hydration.dehydrated` onto a resident file, and the emblem
would then draw "cloud" on a file that holds bytes (`lib.rs:181-184`). This mislabels but is
harmless and matches the framework's *own* view — it would also treat that file as a
placeholder. The emblem cannot be more correct than the mark, and that is acceptable. The
DOWNLOADING state deliberately has *no* forgeable on-disk marker (§What-this-establishes,
point 2); it is derived from the live client set, which no bystander can write.

---

## 5. The edit list, smallest-shippable-slice first

Groundwork before code: the probes in §6 come **first**. Nothing below is asserted by a test
until the measurement it rests on exists. Four slices; the dependency decision (§1.1, §1.4) is
taken once, up front, and is identical for every emblem slice.

### Slice 1 — static two-state emblem. No framework change, no IPC. (Ships first.)

*Product — `OneDriveHydration`:*
1. **`packaging/dolphin/overlay/` (NEW)** — the `KOverlayIconPlugin` `.so`: `CMakeLists.txt`
   (`kcoreaddons_add_plugin`, `KF6::KIOCore`, install `kf6/overlayicon`), the C++ class doing
   `getOverlays` → `QHash` cache → on miss, worker-thread `lgetxattr` → `overlaysChanged`, and
   the state→emblem map (§1.3). *Tests/verification:* the plugin **loads and draws** on this
   Plasma 6 / Dolphin 26.04.3 / KF6 desktop (P2), a placeholder shows the cloud emblem and a
   resident shows the check, and a bystander scan of a placeholder directory fires **zero**
   pre-content events and hydrates nothing (P1 — the gate).
2. **`packaging/icons/`** — port the reserved `onedrive-cloud` (and, if a branded check is
   chosen, `onedrive-synced`) into the hicolor tree; update `packaging/icons/README.md:46-49`
   to record that this milestone now names them.
3. **`packaging/dolphin/install-*.sh` + `docs/DOLPHIN-GROUNDWORK.md`** — install the `.so`,
   report and print the `sudo rm` for the stale `onedrive-overlay.so` (§1.4;
   `DOLPHIN-GROUNDWORK.md:121-142`), and record the dependency decision as taken.

### Slice 2 — aggregate "Downloading N" field. One framework change + bridge + plasmoid.

*Framework — `HydrationAPI` (flag to the framework owner):*
1. **`hydration-client/src/lib.rs`** — the RAII in-flight guard around `provider.fetch`
   (`:227-246`), threaded from an `Arc<AtomicU64>`. *Test:* `serve_raises_and_clears_the_gauge`
   — the gauge reads 1 during a fetch and 0 after, including on the `begin`-error and abort
   paths.
2. **`hydration-client/src/daemon_loop.rs`** — `downloading` on `WatchState` (`:322-331`),
   appended to `line()` (`:339-344`), read in `watch_state()` (`:354-366`), atomic created in
   `run()` and cloned into `Daemon::new` (`:1538`) and the broadcast thread. *Tests:*
   `watch_line_appends_downloading_after_exposures` (key order preserved);
   `an_idle_daemon_reports_downloading_0`.

*Product — `OneDriveHydration`:*
3. **`crates/onedrive-daemon/src/dbus.rs`** — `downloading` on `DaemonState` (`:76-85`);
   `"downloading"` arm in `apply_state_line` (`:105-110`); `Downloading` property (`:448`);
   **new** `DownloadChanged(t)` signal (`:531`); emit both in `publish_state` (`:562-592`).
   *Tests:* mirror `tests/dbus_surface.rs` — assert `StateChanged` stays `(bool,u64,u64,u64)`
   (unchanged pin) and that `downloading` travels on its **own** signal, exactly as the
   credential test does.
4. **`crates/onedrive-daemon/src/bin/onedrive-hydration-dbus.rs`** — no change if
   `downloading` folds into the existing `publish_state`; it rides the same watch line.
5. **`packaging/plasmoid/.../ui/main.qml`** — `property double downloading: 0` (beside
   `:63-66`); pick it up in `readAll`'s GetAll apply block (`:290-299`); add
   `dbusDownloadChanged(n)` to the `SignalWatcher` (beside `dbusStateChanged` at `:383-389`),
   routing through `u64()` (`:246-248`). *Test:* the `{value:n}` wrapper decodes to a number,
   not `[object Object]` (`main.qml:24-27`).
6. **`packaging/plasmoid/.../ui/FullRepresentation.qml`** — one row after the placeholders row
   (`:94-97`): `visible: full.host.downloading > 0`, text `full.host.count(full.host.downloading,
   "file", "files")`, label "Downloading:". Do **not** touch the `presentation` precedence map
   (`main.qml:170-243`) — downloading is informational, not an attention state, and that map is
   pinned against `tray.rs` by `tests/plasmoid_package.rs`.

### Slice 3 — live emblem refresh via the Nextcloud push socket.

*Framework + product:* a per-path status endpoint on the client (modelled on Nextcloud's
`REGISTER_PATH`/`RETRIEVE_FILE_STATUS`/`STATUS`/`UPDATE_VIEW`); the plugin subscribes and emits
`overlaysChanged` on push, removing the relist dependency (§1.5). Gated on P3 showing relist is
insufficient. This is genuinely new IPC — do it after Slices 1–2 prove value.

### Slice 4 — DEFER: the DOWNLOADING third-state emblem, and per-file/byte progress.

Consumes the Slice-3 push channel to carry the client's in-flight *paths* to the plugin
(`onedrive-partial`) and, if wanted, per-file byte progress for whole-object pulls. Blocked on
Slice 3 and on pinning the third-state semantics under span-scoped hydration.

---

## 6. Groundwork: what a probe must measure before code

Ordered by how much rests on it.

- **P1 — `getxattr`/`lgetxattr` fires zero pre-content events, DIRECTLY. The gate.** The whole
  emblem feature rests on it. Sound by principle (`getxattr` cannot reach `rw_verify_area` /
  `fsnotify_file_area_perm`; `DESIGN.md:157-162,1162,1335`) and corroborated by a *stronger*
  call (`probes/seekdata.c`: even `lseek(SEEK_DATA)` fires 0), but **no existing probe does
  `getxattr` specifically under a live PRE_CONTENT mark and counts events** — `probes/` has
  `seekdata.c`, `precontent.c`, `emptyread.c`, `eventtrace.c`, none exercising a metadata-only
  op. Add a `probes/xattrread.c` (or a `getxattr` row in `eventtrace.c`): on a **real mount,
  not tmpfs**, mark it with `hydrationd`, then from a **bystander** process
  `open`+`lgetxattr`+`llistxattr` across a directory of placeholders, assert **0** events, no
  hydration, and correct not-zeros for a control read — on btrfs, ext4-128, and xfs (xfs is
  unmeasured even for `seekdata`, `DESIGN.md`). Ship nothing until this is green: a wrong answer
  hydrates or deadlocks 166k files.
- **P2 — a trivial `KOverlayIconPlugin` `.so` loads and draws on this desktop.** Same evidence
  bar the actions half met with `probes/servicemenu-match.cpp` (`DOLPHIN-GROUNDWORK.md:33-56`):
  build a do-nothing plugin, confirm Dolphin 26.04.3 / KF6 / Plasma 6 loads it, resolves the
  install path, and renders a returned emblem — and that it does not double up with the stale
  `onedrive-overlay.so` (`DOLPHIN-GROUNDWORK.md:121-142`).
- **P3 — refresh triggers, empirically.** Measure: (a) does a hydrate (content rewrite, size/
  mtime move) make Dolphin re-poll `getOverlays` on the next relist? (b) does an eviction
  (inode swap + re-mark)? (c) does `overlaysChanged(QUrl)` refresh a **single** item with no
  relist? (d) confirm an xattr-only change does **not** trigger a re-poll (expected — ctime
  only). This decides whether Slice-1 relist-only is tolerable or Slice 3 is mandatory sooner.
- **P4 — `lgetxattr` latency on the main thread.** Across a large directory (e.g. 5000 visible
  entries, warm and cold inode cache), to decide whether the `QHash` cache alone suffices or the
  async-miss worker is load-bearing. Dolphin already `stat`s every listed file, so the marginal
  cost is one `lgetxattr` per visible item — likely microseconds warm; the cold huge directory
  is the case to measure.
- **P5 — fetch cadence for Feature 2.** On the live rig, do typical hydrations open and close
  inside one 1-second broadcast tick (`daemon_loop.rs:1521`)? If short fetches routinely slip
  between samples, the field is a "sustained download" indicator only — say so in the UI and the
  commit — and confirm the span-scoped-fetch fact (§3.3) so no one wires a whole-file bar.

---

## 7. What this deliberately does not do

- **No new on-disk "downloading"/"building" xattr.** It was removed on purpose (forgeable,
  fail-open, made a placeholder serve zeros — `lib.rs:190-198`); the third state comes from the
  live client set, never the filesystem.
- **No per-file download list or percentage in v1.** The client serves one span-scoped fetch at
  a time (`daemon_loop.rs:1527-1531`, `lib.rs:196-274`); a list is ≤1 entry and a whole-file bar
  is dishonest (§3.3). Deferred to Slice 4.
- **No change to `StateChanged`'s pinned `(bool,u64,u64,u64)` shape.** `downloading` is a new
  property + new signal, mirroring the credential precedent (`dbus.rs:22-28,524-532,546-560`).
- **No `KVersionControlPlugin`, no sentinel file at the sync root, no shell/QML emblem route.**
  §1.1.
- **No thumbnailer fix.** The emblem is safe; thumbnail generation over cloud-only files is a
  separate hydration hazard flagged for the roadmap (§4.1), not solved here.
- **No second download-tracking mechanism.** Feature 1's third state and Feature 2's field are
  one signal at two granularities (§2.2, §3.1).

---

# Critique of the above

**(a) Claims argued rather than measured.**

- **The whole emblem feature rests on P1, which is unrun.** "`getxattr` fires no pre-content
  event" is a strong argument from the kernel hook set (`DESIGN.md:157-162`) and a
  stronger-call measurement (`seekdata.c`), but it is exactly the class `CLAUDE.md` says has
  fooled eight reviews, and the specific op — `getxattr` under a live mark, from a bystander,
  counted — has never been measured. The document says "gate on P1," which is correct, but until
  it runs, a plugin that `lgetxattr`s thousands of visible files rests on an unmeasured claim,
  and that should be uncomfortable. `llistxattr` in particular is asserted alongside `getxattr`
  with even less basis — it is a different syscall and should be in the same probe, not assumed.
- **"Relist re-queries a hydrate/eviction" is assumed, not shown.** §1.5 leans on Dolphin
  re-polling `getOverlays` when content changes, but P3 is precisely the measurement that a
  hydrate's size/mtime move, or an eviction's inode swap, actually triggers a relist on *this*
  KIO build. If it does not, Slice 1 ships a badge that goes stale until F5 — usable, but not
  what the table implies — and Slice 3 becomes mandatory, not optional. The doc orders Slice 1
  first *on the assumption P3 is favourable*, which is backwards from "measure first."
- **The 1 Hz caveat is described, not quantified.** §3.4 says short fetches may be missed but
  cites no fetch-duration distribution; P5 is deferred. Shipping "Downloading N" before P5 means
  the field's *reliability* is unknown — it may be near-useless for the common small hydration
  and only ever light up on large pulls, which is a different feature than "the client is
  downloading."

**(b) Structural gaps the edit list does not close.**

- **Nothing type-enforces that `getOverlays` stays content-free.** §4.1 makes "only `lgetxattr`,
  never a read" a convention and a test, but a future maintainer adding a "peek the first bytes
  to pick a MIME-specific emblem" would reintroduce the §6a-ter hazard, and the C++ side has no
  equivalent of the framework's `safe_join`/type discipline to stop it. This is the same
  residual the Keep-on-Device and auto-eviction groundwork both flagged and left open — here it
  is worse, because the hazard is a *read that hydrates*, and it lives in a repo whose CI cannot
  even see the C++ graph (`DOLPHIN-GROUNDWORK.md:20`).
- **The RAII gauge guard is specified as prose, not designed.** §3.1 correctly identifies that a
  hand-written inc/dec leaks on the `conn.begin(...)?` early return, and prescribes a guard — but
  where the guard type lives, whether it holds the `Arc` or a borrow, and whether it correctly
  decrements when `serve` returns `Err` up the stack (dropping the guard mid-`match`) is left to
  the implementer. A guard that decrements in the wrong scope re-creates exactly the stuck
  "downloading=1" it exists to prevent.
- **The resident-emblem asset is undecided.** §1.3 offers "Breeze `vcs-normal` or a new
  `onedrive-synced`" and calls it a one-liner, but the donor set genuinely lacks a resident
  badge (`icons/README.md:46-49`), so "port the reserved emblems" does not cover the check —
  Slice 1's icon step is under-scoped by one asset, and "one line since `getOverlays` takes any
  name" hides that a branded check needs an SVG drawn to the donor's 24-grid geometry and
  palette, which is design work, not a line.

**(c) Smaller drift.**

- **`downloading` as a count is really a boolean today.** §2.1 exposes a `u64` count for
  forward-compatibility with a parallelized fetcher that does not exist, so the plasmoid's
  `count(n, "file", "files")` will only ever render "1 file" or be hidden. That is fine, but the
  pluralization and the "N" framing quietly over-promise concurrency the framework does not have
  — if a reader takes "Downloading N files" at face value they will wonder why it is never 2.
- **Feature 2's field can outlive its cause under the 1 Hz dedup.** If a fetch ends between
  broadcasts and the very next state is otherwise identical, the deduped broadcast
  (`daemon_loop.rs:1521`, `WatchState: PartialEq`) still sends because `downloading` changed
  1→0 — so this is fine — but the inverse edge (a fetch that starts *and* ends within one tick
  changing nothing) means the field can stay 0 through a real download. The doc notes the miss
  but not that the dedup makes "downloading" the *only* key that is expected to flip twice per
  event, which stresses the change-detection path in a way the three static counters never did.
- **The stale `onedrive-overlay.so` is a runtime collision the install script can only *warn*
  about.** §1.4 reuses the existing "report and print `sudo rm`" stance, but unlike the
  servicemenu case (where the stale plugin draws nothing harmful), two live overlay plugins that
  both return emblems for the same files is a visible double-badge, and "the machine owner's
  call" (`DOLPHIN-GROUNDWORK.md:132-136`) means Slice 1 can ship into a desktop that renders two
  clouds per file until the user runs a command by hand. Whether Dolphin merges or stacks two
  plugins' overlays is itself unmeasured.
