# The Dolphin actions

Two inverse context-menu actions on the selected files: **Free Up Space**
evicts them back to placeholders, and **Keep on Device** pins them so eviction
skips them and pulls their content down now. Shipped as data — one KIO
servicemenu `.desktop` and a POSIX shell wrapper per action — for the same
reason the tray is a StatusNotifierItem and the flyout is QML: the file manager
already knows how to draw a menu, and a toolkit would buy nothing. Zero new Rust
dependencies; `cargo deny check` sees an unchanged graph.

Both are file-only, sharing the one measured `all/allfiles` matching. Keep on
Device works on a file by `pin`-ning it and then asking `onedrive-hydrationctl
hydrate` to read it down; the wrapper never opens the file itself, so the read
that hydrates stays in the one process that is neither the daemon nor the helper
(§6a-ter). A directory pin — which the daemon and the `pin` verb already
support — and folder-recursive hydration are a deliberate follow-up: the first
needs `inode/directory` matching that `probes/servicemenu-match.cpp` has not yet
measured, the second an enumeration verb that is not built.

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

It writes two files under `$XDG_DATA_HOME` (default `~/.local/share`):

| | |
|---|---|
| `kio/servicemenus/onedrive-hydration.desktop` | the menu entry (both actions) |
| `onedrive-hydration/free-up-space.sh` | the Free Up Space wrapper |
| `onedrive-hydration/keep-on-device.sh` | the Keep on Device wrapper |

## Measured on this KIO build, not taken from documentation

With `probes/servicemenu-match.cpp`, which builds the real `KFileItemActions`
menu and prints it. Full detail in `docs/DOLPHIN-GROUNDWORK.md`.

* `MimeType=all/allfiles;` reaches a regular file of any mimetype, and does
  **not** reach a directory — which is what is wanted, because `evict` takes a
  file and there is no bulk-evict to offer on a folder.
* The entry survives a multi-file selection, so `%F` and the wrapper's loop
  are honest. A mixed file+directory selection matches nothing at all.
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

## What is deliberately not here

**Status overlay emblems.** The other half of the roadmap item. There is no
data-only path — `KOverlayIconPlugin` and `KVersionControlPlugin` are both
compiled C++ — so it is a real dependency decision (CMake, Qt6, KF6 in a Cargo
workspace, and a `.so` installed as root) rather than more of the same work.
`docs/DOLPHIN-GROUNDWORK.md` records what it would need, including the per-file
xattrs that are already on disk and the `st_blocks` trap it must not fall into.

**A folder action, for now.** Both entries are file-only. Free Up Space stays
that way on purpose — recursing in shell would invent a bulk evict the daemon
does not offer, with none of its judgment about what is safe. Keep on Device
*could* grow one, since a directory pin is one `setxattr` the daemon already
does and hydration is safe to recurse; but it needs `inode/directory` servicemenu
matching (unmeasured on this KIO build) and a content-free enumeration verb to
list what to pull down. Both are enumerated in `HydrationAPI`'s
`docs/KEEP-ON-DEVICE-GROUNDWORK.md` (§3) and left to a follow-up rather than
shipped as an unverified claim or a shell tree-walk.

**Anything to do with `/usr/lib/qt6/plugins/kf6/overlayicon/onedrive-overlay.so`.**
An unpackaged overlay plugin from the donor client may still be installed
system-wide, reading `user.onedrive.syncstate` — an xattr this product never
writes (measured: 0 of 400 files on the live mount). `install-servicemenu.sh`
reports it and prints the removal command; it does not run it. That file is
root-owned and outside the per-user scope this script installs into.
