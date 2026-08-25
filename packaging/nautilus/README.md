# GNOME Files (Nautilus) integration

The GNOME sibling of `packaging/dolphin`: "Free Up Space" and "Keep on
Device" as Nautilus *scripts* — POSIX shell under
`~/.local/share/nautilus/scripts`, no toolkit, no new dependency. Install per
user with:

```text
packaging/nautilus/install-nautilus-scripts.sh --mount "$HOME/OneDrive"
```

They appear under right-click → **Scripts** on any selection. Nautilus names
a script by its filename and offers no filtering at all — not even KIO's
mimetype filter — so the entries exist on every file on the system and the
sync-root containment lives inside the scripts, which refuse anything
outside the mount by name.

The rules the Dolphin wrappers established hold here byte for byte, because
they come from the daemon, not the desktop:

- **Nothing ever opens the target.** On this mount a read is what hydrates a
  placeholder, so the scripts perform path operations only; the deliberate
  read lives inside `onedrive-hydrationctl hydrate`.
- **Replies are read, exit statuses are not.** `onedrive-hydrationctl` exits
  0 for a `kept:` refusal, so a script that trusted `$?` would count a kept
  file as freed. The reply prefixes are pinned to the Rust parser by
  `crates/onedrive-daemon/tests/nautilus_package.rs`.
- **Folders expand through the daemon.** Keep on Device pins the folder as
  one mark and pulls its dehydrated files from the daemon's `pending`
  listing; Free Up Space releases the folder's own pin, then evicts per
  file. A `zenity --progress` dialog paces a large pull and closing it stops
  the pull — never the pin that already happened.
- **A directly pinned file is un-kept by Free Up Space**; one pinned through
  an ancestor folder is left alone and the daemon's refusal, naming the
  folder, is shown verbatim.

Results arrive as a passive `notify-send` on success and a modal
`zenity --error` (no Pango markup) on refusal; with neither installed the
text goes to stderr, which at least reaches the journal.

What GNOME does not get, said plainly: per-file cloud/on-device **emblems**.
The Dolphin side ships them as a compiled KF6 `KOverlayIconPlugin`; GNOME
Files has no equivalent overlay-plugin API to port it to. The actions work
without them — the honest gap is that residency is not visible at a glance
in the file manager, only through the tray and `onedrive-hydrationctl
status`.
