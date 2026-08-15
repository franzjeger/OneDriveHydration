# Roadmap

## M0: trustworthy skeleton

- [x] Separate repository and Rust workspace
- [x] Revision-pinned HydrationAPI dependency
- [x] GraphAccess daemon wiring
- [x] Formatting, lint, test, docs and dependency-policy CI
- [x] Publish the declared MIT OR Apache-2.0 license texts

## M1: enroll and identify the account

- [x] Device-code enrollment using the shared TokenCache
- [ ] PKCE/browser enrollment threat-model review
- [x] Resolve `/me/drive` instead of requiring `--drive-id`
- [x] Store credentials through Secret Service/keyring
- [ ] Live Graph smoke test using a dedicated non-production tenant

## M2: production transfers

- [x] Streaming fetch provider with explicit credential-safe redirect handling
- [x] In-flight HTTP range/resume support without exposing partial content
- [x] Resumable downloads with bounded retry and strict Content-Range validation
- [x] Hard-fail QuickXorHash verification
- [x] Download throttling and disconnect fault-injection tests
- [ ] Process-restart fault-injection tests

## M2.5: complete sync semantics — release blocker

- [x] Pin the HydrationAPI stabilization revision
- [x] Separate cTag from QuickXorHash while verifying both for full hydration
- [x] Preserve atomic-save identity and its base cTag
- [x] Propagate same-folder file rename as a conditional item-ID operation
- [x] Carry the recorded cTag into local deletion
- [x] Pair split rename events before drawing a destructive conclusion
- [ ] Give local folders durable cloud identity
- [ ] Create and retain empty folders in both directions
- [ ] Move files between parents by destination folder ID
- [ ] Implement guarded local folder create, rename, move and delete
- [ ] Pass the two-device conflict and process-restart matrix in
      [the sync correctness gate](SYNC-ACCEPTANCE.md)
- [ ] Pass the complete matrix against a dedicated non-production tenant

## M3: Linux product shell

Feature work in this milestone is frozen until M2.5 is complete. Changes needed
to expose an honest sync error or unsupported operation are still in scope.

- [x] Owner-only local status and eviction CLI
- [x] D-Bus control surface
- [x] Revision-matched Linux binary payload, including privileged helper
- [x] Validated systemd installer and units
- [x] StatusNotifierItem tray icon and menu, signal-driven
- [x] Flyout: system-tray plasmoid with eviction, signal-driven
- [x] Credential state on the D-Bus surface (property + change signal), shown by
      tray and flyout, with adopt-on-restart of a fresh `pkce-enroll.py` sign-in
- [ ] In-product (re-)enrollment — blocked on M1's PKCE threat-model review; until
      then the surfaces name `tools/pkce-enroll.py` and deliberately offer no
      sign-in button
- [x] Dolphin action: "Free Up Space" as a KIO servicemenu, shipped as data with
      no new dependency; the entry's matching was measured with
      `probes/servicemenu-match.cpp` rather than taken from documentation
- [ ] Dolphin status overlays — no data-only path exists: `KOverlayIconPlugin`
      and `KVersionControlPlugin` are both compiled C++, so this is a dependency
      decision (CMake, Qt6 and KF6 in a Cargo workspace, and a `.so` installed as
      root) and not more of the same work. The per-file xattrs it would read are
      already on disk; `docs/DOLPHIN-GROUNDWORK.md` has the measurements, the
      `st_blocks` trap it must avoid, and the stale donor plugin it would collide
      with

### The neighbour-reader hydration hazard (measured live, 2026-08-15)

`DOWNLOAD-VISIBILITY-GROUNDWORK.md` flagged it and deferred it here: on Linux
there is no cloud-filter API, so **any** process that reads a placeholder's
*content* hydrates it — a read is a read, and `FAN_PRE_ACCESS` cannot tell a
user's `cat` from a background indexer's. The emblem plugin is safe (xattr
metadata only, no `open`), but the file manager's own **thumbnail generation is
not**: rendering a preview reads the file, so browsing the sync folder with
previews on downloads every placeholder it draws.

Confirmed on the live rig by isolation: an evicted file stays cloud-only only
when Dolphin is closed *and* every `kioworker` running `kf6/kio/thumbnail.so` is
killed — the thumbnail worker even lingers after Dolphin exits, draining its
queue, which is what made "Free Up Space" look like a no-op (the file re-hydrated
seconds later) and made a mostly-placeholder tree read as "everything is
downloaded." Baloo was **not** implicated: it was disabled, and `~/OneDrive` was
already in its `exclude folders`.

- [x] **Mitigation (documented, applied per-machine): previews off for the sync
      tree.** The reliable, recursive lever in Dolphin is
      `GlobalViewProps=true` + a global view-property with `PreviewsShown=false`
      — the native "uncheck Show Previews → Apply to All Folders", GUI-reversible.
      This matches Windows' cloud-only behaviour: a type icon + cloud badge, never
      a content thumbnail. Per-folder `.directory` is **not** recursive and would
      write into the sync root (and upload) unless `.directory` is added to
      `.hydration-ignore`, so it is the weaker option.
- [ ] **Open question for the installer:** whether the product should apply the
      previews-off setting itself (intrusive — it changes the user's Dolphin
      globally), offer it, or only document it. Other content readers (backup
      agents, antivirus, `updatedb`, a second indexer) remain out of the product's
      reach and belong in user-facing docs as the same class of hazard.
