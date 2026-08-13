# Dolphin integration: what was measured, and what it costs

Groundwork for the roadmap's "Dolphin actions and status overlays". The
actions half is built (`packaging/dolphin/`). The overlays half is not, and
this document is why: it needs a dependency decision this repository has twice
refused to take by accident, and it should not be taken by accident here
either.

Everything below was measured on the deployment machine — Dolphin 26.04.3,
KF6, Plasma 6 — with `probes/servicemenu-match.cpp` and with `getfattr`
against the live sync root. Where something was *not* measured, it says so.

## The two halves are not the same kind of work

| | Actions | Status overlays |
|---|---|---|
| Mechanism | KIO servicemenu — a `.desktop` file | `KOverlayIconPlugin` — a compiled `.so` |
| Language | data, plus POSIX shell | C++ against Qt6 and KF6 KIO |
| Build | none | CMake, in a Cargo workspace |
| `cargo deny` | unchanged graph | cannot see it at all |
| CI | already covered | would need a second toolchain |
| Install | per user, `$XDG_DATA_HOME` | a `.so` under `/usr/lib`, as root |

There is no data-only path to overlay emblems. The interface is
`/usr/include/KF6/KIOCore/koverlayiconplugin.h`, a C++ class with a virtual
`getOverlays(const QUrl &)`; the other route, `KVersionControlPlugin`
(`libdolphinvcs.so`), is also C++. Nextcloud's Dolphin emblems are exactly
such a compiled plugin. This is the same wall the tray and the flyout each
hit and walked around — the tray by speaking StatusNotifierItem over the zbus
already in the tree, the flyout by shipping QML as data — and here there is no
way around it, only through it.

## What was measured for the actions half

`probes/servicemenu-match.cpp` builds the real `KFileItemActions` menu for a
given set of paths and prints what a context menu would contain. Against a
control that produced no actions at all:

* `MimeType=all/allfiles;` **reaches a regular file** of any mimetype
  (measured on `text/plain`). Nothing installed on this machine used
  `all/allfiles`, so until the probe ran this was a documented convention and
  not a measurement.
* It **does not reach a directory** — only the unrelated `inode/directory`
  entry appeared there. This is what makes the value correct rather than
  merely working: the control socket's `evict` verb takes a file, and there is
  no bulk-evict to offer on a folder.
* It **survives a multi-file selection**, so `Exec=… %F` and the wrapper's
  loop are honest.
* A **mixed file+directory selection matches nothing at all** — the entry
  disappears rather than appearing with an unclear target.
* A servicemenu dropped into `$XDG_DATA_HOME/kio/servicemenus` is found by a
  freshly started process **with no `kbuildsycoca6` run and no cache rebuild**,
  on a cold cache that had never seen it. `install-servicemenu.sh` therefore
  tells nobody to rebuild anything. Whether an *already open* Dolphin window
  rescans the directory was **not measured**, and the script says only that
  reopening a window is enough if it does not appear.

### The limitation that shaped the wrapper

**KIO cannot restrict a servicemenu entry by path.** `MimeType` is the only
filter; there is no path condition. Measured: the shipped entry appears on a
file outside the sync root exactly as it does on one inside. So the entry is
present on every file on the system, and the check has to live in the wrapper,
which refuses with a specific message naming the sync root.

A `KFileItemActionPlugin` could filter by path and show the entry only inside
the sync root — but that is a compiled C++ plugin, which is the same decision
as the overlays. The two halves of the roadmap item turn out to be one
dependency question asked twice.

### Traps the wrapper encodes

* **`onedrive-hydrationctl` exits 0 for a refusal.** It exits 1 only for
  `error:` and `unknown command:`; a `kept:` reply is a successful exit. A
  wrapper that branched on `$?` would count kept files as freed while the
  daemon was explaining why it had not touched them.
  `crates/onedrive-daemon/tests/dolphin_package.rs` derives the reply prefixes
  from `parse_evict_reply` so rewording the protocol fails a test.
* **Reading the file is what hydrates it.** The wrapper does path operations
  only — `readlink -f` and shell string work — and never opens its argument. A
  `file` or `head` call to "check" a target would fill the very placeholder the
  user asked to empty, before asking the daemon to empty it. A test asserts no
  reader command appears in command position.

## What the overlays would need, if they are built

Per-file state is already on disk and needs no daemon round-trip. Measured on
the live mount:

```
user.hydration.dehydrated="1"      # present only on placeholders
user.hydration.id=...              # on every synced file
user.hydration.etag=...
user.hydration.stamp=...
```

A 499 MB placeholder carried `dehydrated="1"` with `st_blocks` 0; hydrated
files carried no `dehydrated` xattr. So the emblem states are readable, and
the plugin would be a cheap `getOverlays` that returns an icon name.

Three things it would have to get right:

1. **Do not use `st_blocks`.** HydrationAPI's own `CLAUDE.md` records this as a
   production bug that survived several rounds of review: block counts report
   the same number for an empty file and for a placeholder, and on ext4 with a
   small inode a placeholder is charged a block for its xattrs. The marker is
   the `dehydrated` xattr; the framework's own answer is `placeholder::holds_data`
   (`SEEK_DATA`).
2. **`user.hydration.dehydrated` is owner-writable.** Fine for drawing an
   emblem, which is not a security boundary — but it must not become an input
   to anything that is.
3. **Refresh.** `KOverlayIconPlugin::overlaysChanged` exists for pushing
   updates; wiring it to the daemon's `StateChanged` would be a new coupling,
   and the alternative — emblems that only refresh when Dolphin relists — should
   be measured before it is assumed to be bad.

The donor client's three 16 px emblems (`onedrive-cloud`, `onedrive-partial`,
`onedrive-upload`) are reserved for this work in `packaging/icons/README.md`
and deliberately not shipped until it names them.

## The stale plugin on this machine

`/usr/lib/qt6/plugins/kf6/overlayicon/onedrive-overlay.so` is installed
system-wide, root-owned, and **owned by no package**. It is a
`KOverlayIconPlugin` (`org.kde.overlayicon.onedrive`) left over from the donor
client, and it reads `user.onedrive.syncstate`.

Measured on the live sync root: **0 of 400 files carry that xattr**; the only
names in use are `user.hydration.*`. So it loads into every Dolphin process on
this machine and draws nothing.

`install-servicemenu.sh` reports it, explains what it reads, and prints the
`sudo rm` line without running it. It is a root-owned file outside the
per-user scope this product installs into, and removing it is the machine
owner's call — the same reason the systemd installer prints the subvolume
command instead of running it.

Note for whoever builds the overlay half: that file is also a *name* collision
waiting to happen. A new plugin installed beside it would give Dolphin two
overlay sources for the same files — the same shape as the tray/plasmoid
collision that `--tray` now refuses, and worth deciding before it ships rather
than after.
