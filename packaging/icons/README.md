# Icons

The tray icons `onedrive-hydration-tray` publishes by name, laid out as a
hicolor icon theme fragment so `install-icons.sh` (or a future package) can
copy the tree verbatim:

| Icon name | Tray state |
|---|---|
| `onedrive-hydration-synced` | daemon running, nothing unsent, no exposures |
| `onedrive-hydration-unsent` | local changes not yet uploaded |
| `onedrive-hydration-exposed` | another mount exposes the sync files (the warning state) |
| `onedrive-hydration-stopped` | daemon or state service not running |
| `onedrive-hydration` | the application icon, used in the tooltip |

`./install-icons.sh` installs them for the current user
(`$XDG_DATA_HOME/icons/hicolor`, default `~/.local/share/icons/hicolor`) and
notifies running KDE processes, which do not rescan icon themes on their
own. StatusNotifier hosts resolve the names through the desktop's icon
theme; until this has run, Plasma falls back to resolving the item's `Id`
(measured), so the tray shows the application icon for every state.

## Provenance and attribution

All five are ported from
[OneDriveForLinux](https://github.com/franzjeger/OneDriveForLinux), the
donor client this repository's README names for product features.

The donor kept its four SVG assets and its tray art as *different things*,
and the port preserves that distinction rather than mapping four files onto
four states:

- Its tray never used the SVG assets. It rasterized one cloud silhouette
  with a colored lower-right state badge at runtime
  (`crates/tray/src/icons.rs`, tiny-skia). The four state icons here are
  that design transcribed to SVG — same 24-grid geometry, same palette
  (`#E8EDF3` cloud, `#57B183` ok, `#5AA2DD` activity, `#D0716A` alarm,
  `#8E9CAC` inactive, `#10151C` badge ring) — plus the dark rim from its
  emblem series so the light cloud survives light panels. Two states have
  no donor badge of their own: "unsent" reuses the donor's upload arrow on
  the activity blue, and "exposed" pairs the donor's alarm red with its
  exclamation glyph, because the exposure hazard is a warning to a person,
  not a failed operation.
- `assets/onedrive-linux.svg`, the launcher icon, is ported unchanged as
  `onedrive-hydration.svg`.
- The donor's other three SVGs (`onedrive-cloud`, `onedrive-partial`,
  `onedrive-upload`) are 16 px file-manager overlay *emblems* — per-file
  state, not application state. The completed Dolphin plugin uses the desktop
  theme's `cloud-download` and `emblem-success` icons instead, so those donor
  assets remain unnecessary and are not shipped.

The donor repository declares no license of its own. Both repositories have
the same author, and these ported and derived files are distributed under
this repository's MIT OR Apache-2.0 terms with that authority; this notice
is the attribution the port is required to retain.
