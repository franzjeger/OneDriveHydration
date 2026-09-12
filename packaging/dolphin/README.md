# The Dolphin actions

Two inverse context-menu actions on selected files and folders: **Free Up Space**
evicts them back to placeholders, and **Keep on Device** pins them so eviction
skips them and pulls their content down now. Shipped as data — one KIO
servicemenu `.desktop` and a POSIX shell wrapper per action — for the same
reason the tray is a StatusNotifierItem and the flyout is QML: the file manager
already knows how to draw a menu, and a toolkit would buy nothing. Zero new Rust
dependencies; `cargo deny check` sees an unchanged graph.

Separate file and folder menu entries expose both actions. The folder Free Up
Space wrapper enumerates files and calls the daemon's `evict` operation for
each one, preserving its per-file checks. Keep on Device
works on a file by `pin`-ning it and asking `onedrive-hydrationctl hydrate` to
read it down, and on a folder by pinning it once (the pin protects the subtree)
and pulling its dehydrated files down one at a time via the daemon's `pending`
enumeration. The wrapper never opens a file itself, so the read that hydrates
stays in the one process that is neither the daemon nor the helper (§6a-ter).

Install per user, after `../icons/install-icons.sh`:

```
./install-servicemenu.sh --mount ~/OneDrive --bin-dir /usr/local/bin
```

Both values are baked into the generated files, the way the systemd installer
bakes facts into units and for the same reason: this action has no
configuration file and no session to read one from, so a wrapper that guessed
either would guess it silently. The script refuses — naming the fact that was
wrong — if the sync root does not exist or `onedrive-hydrationctl` is not
executable where it was told. A missing CLI would otherwise produce a menu
entry that fails only when clicked.

It writes these files under `$XDG_DATA_HOME` (default `~/.local/share`):

| | |
|---|---|
| `kio/servicemenus/onedrive-hydration.desktop` | the file entry (both actions) |
| `kio/servicemenus/onedrive-hydration-folder.desktop` | the folder entry (both actions) |
| `onedrive-hydration/free-up-space.sh` | the Free Up Space wrapper |
| `onedrive-hydration/free-up-space-folder.sh` | the folder Free Up Space wrapper |
| `onedrive-hydration/keep-on-device.sh` | the Keep on Device wrapper |

## Measured on this KIO build, not taken from documentation

With `probes/servicemenu-match.cpp`, which builds the real `KFileItemActions`
menu and prints it. Full detail in `docs/DOLPHIN-GROUNDWORK.md`.

* `MimeType=all/allfiles;` reaches a regular file of any mimetype, and does
  **not** reach a directory; folder actions have their own entry.
* `MimeType=inode/directory;` (the folder entry, measured on KIO 6.28) reaches a
  directory and **not** a regular file, so the folder entry never doubles the
  file entry's Keep on Device. It survives a multi-directory selection.
* The entry survives a multi-file selection, so `%F` and the wrapper's loop
  are honest. A mixed file+directory selection matches nothing at all — for
  either entry — so a mixed selection offers nothing rather than the wrong thing.
* A dropped-in servicemenu is picked up by a freshly started process with no
  `kbuildsycoca6` and no cache rebuild, so the script tells nobody to rebuild
  anything. Whether an already-open window rescans was not measured, and is
  not claimed.

## What the wrapper has to get right

**KIO cannot filter a servicemenu by path.** `MimeType` is the only condition
— measured: the entry appears on files outside the sync root exactly as it
does inside. So the entry exists on every file on the system and the wrapper
refuses, naming the sync root, when asked about one that is not in it. The fix
that would hide the entry entirely is a compiled `KFileItemActionPlugin`,
which is the same dependency decision as the status overlays.

**`onedrive-hydrationctl` exits 0 when the daemon refuses.** Only `error:` and
`unknown command:` exit 1; a `kept:` reply is a successful exit. The wrapper
parses the reply text, never `$?`, or it would report kept files as freed.
`crates/onedrive-daemon/tests/dolphin_package.rs` derives those prefixes from
`parse_evict_reply` so the protocol and the shell cannot drift apart quietly.

**Reading a file is what hydrates it.** The wrapper only ever does path
operations on its arguments and never opens them; a `file` or `head` call to
"check" a target would fill the placeholder the user asked to empty. A test
asserts no reader command appears in command position.

Results go through `kdialog`, falling back to `notify-send` and then stderr,
because Dolphin runs the action detached with no terminal. Success is a
passive popup; anything the daemon declined is modal and quoted verbatim, the
way the flyout quotes `Error.Kept` rather than flattening it.

## The status overlay emblems (`overlay/`)

The compiled KF6 `KOverlayIconPlugin` draws these badges inside the configured
OneDrive roots:

| Badge | File | Folder |
|---|---|---|
| Cloud (`cloud-download`) | Online-only placeholder | Contains observed online-only files |
| Green check (`emblem-success`) | Local content matches its last confirmed content stamp | All inspected content matches, and the entire bounded scan completed |
| Arrows (`view-refresh`) | Local content differs from its last confirmed stamp | Contains an observed local content change |
| Question mark (`emblem-question`) | No reliable content status available | Unknown entries or scan limit reached without a more specific observed state |

The green check uses `user.hydration.stamp`, cloud identity, and the current
size/mtime. Residency alone never earns a check. The stamp records content
confirmed by hydration or upload; it does **not** establish that renames,
deletions, remote changes, or conflicts have all settled. Arrows indicate a local
content change, not proof that a transfer is currently active. New or ignored
files without a stamp remain unknown.

The plugin reads only metadata (`lstat` and `lgetxattr`) and directory entries,
never file content, so querying a placeholder does not download it. Folder
inspection stops at 128 entries or four directory levels; a partial scan never
earns a green check. An observed local change takes precedence over an observed
cloud-only file. A cloud badge describes observed availability and does not
certify the rest of a large folder.

Roots are listed in `$XDG_CONFIG_HOME/onedrive-hydration/overlay-roots`.
Outside them the plugin adds no badges. Metadata watches on up to 512 recently
requested items refresh badges after uploads even when size and mtime stay the
same. Servicemenu actions also announce changes through `KDirNotify`; child
changes refresh parent badges up to the configured root.

Install it separately from the servicemenu, because the `.so` must land in the
*system* Qt plugin dir — measured: a `~/.local/lib/qt6/plugins` plugin is not
searched by Dolphin, only the system dir and `$QT_PLUGIN_PATH`:

```
./overlay/install-overlay.sh --mount ~/OneDrive
```

That builds the plugin (needs cmake, Qt6, and KF6 dev packages), installs it
system-wide (sudo), writes the roots config, and removes the donor client's
overlay plugin — which reads `user.onedrive.syncstate` and, now that this product
ships an overlay of its own, would draw a second, wrong badge on every file. This
plugin uses Breeze's built-in icons listed above. Restart Dolphin after
installation to load the plugin.

The CMake/CTest suite loads the real plugin against scratch files and checks
content states, folder limits, root scoping, parent refresh, and xattr-only
upload completion. It does not modify a live OneDrive account:

```
cmake -S overlay -B /tmp/onedrive-overlay-tests
cmake --build /tmp/onedrive-overlay-tests
ctest --test-dir /tmp/onedrive-overlay-tests --output-on-failure
```

## Integrated installation and jobs

Prefer `../install-desktop.sh` for a Plasma desktop. It installs a dynamic
`KAbstractFileItemActionPlugin` under `kf6/kfileitemaction`; its OneDrive submenu
is scoped to configured roots and accepts mixed file/folder selections. These
actions start jobs through the owner-checked D-Bus service. The panel shows job
progress, errors, and cancellation after the current file. Static servicemenus
and shell wrappers remain a standalone fallback. Their folder action now reports
actual failures and uses kdialog for progress/cancellation when available.

A clean file or folder protected by its own or an ancestor's pin uses
`onedrive-hydration-pinned`, a filled green circle. Pin changes refresh badges for
recently requested descendants. Unknown and cloud-only states keep their own
badges until content is known to be resident and clean.
