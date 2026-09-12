# Roadmap

## M0: trustworthy skeleton

- [x] Separate repository and Rust workspace
- [x] Revision-pinned HydrationAPI dependency
- [x] GraphAccess daemon wiring
- [x] Formatting, lint, test, docs and dependency-policy CI
- [x] Publish the declared MIT OR Apache-2.0 license texts

## M1: enroll and identify the account

- [x] Device-code enrollment using the shared TokenCache
- [x] PKCE/browser enrollment threat-model review, with the accepted design
      implemented as a retained IPv4 loopback listener, S256/state validation,
      direct Secret Service storage and no plaintext handoff
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
- [x] Give local folders durable cloud identity
- [x] Create and retain empty folders in both directions
- [x] Move files between parents by destination folder ID
- [x] Implement guarded local folder create, rename, move and delete
- [ ] Pass the two-device conflict and process-restart matrix in
      [the sync correctness gate](SYNC-ACCEPTANCE.md)
- [ ] Pass the complete matrix against a dedicated non-production tenant

## M3: Linux product shell

The original product-shell scope is implemented. M4 tracks the remaining desktop
product work; M2.5's correctness gate still blocks production readiness.

- [x] Owner-only local status and eviction CLI
- [x] D-Bus control surface
- [x] Revision-matched, checksummed Linux release payload, including all product
      binaries, the privileged helper, desktop assets, manifest and tagged publishing
- [x] Validated systemd installer and units
- [x] StatusNotifierItem tray icon and menu, signal-driven
- [x] Flyout: system-tray plasmoid with eviction, signal-driven
- [x] Credential state on the D-Bus surface (property + change signal), shown by
      tray and flyout, with owner-only restart notification after fresh enrollment
- [x] In-product (re-)enrollment: explicit flyout Sign in, browser/PKCE by default
      in the daemon CLI, retained device-code fallback, and terminal outcome reporting
- [x] Dolphin actions: "Free Up Space" and "Keep on Device" for files and folders
      as KIO servicemenus, shipped as data with no new dependency; entry matching was
      measured with
      `probes/servicemenu-match.cpp` rather than taken from documentation
- [x] Dolphin status overlays: compiled KF6 `KOverlayIconPlugin`, xattr-only reads,
      targeted refresh after residency changes, configured-root scoping and removal
      of the stale donor overlay collision

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
- [x] **Installer policy:** document and print the previews-off recommendation;
      never silently mutate Dolphin's global preference. Other content readers
      (backup agents, antivirus, `updatedb`, a second indexer) remain out of the
      product's reach and belong in user-facing docs as the same class of hazard.

## M4: desktop experience

- [x] Distinguish active uploads, downloads, cloud checks and queued changes in both trays
- [x] Indeterminate activity instead of queue-derived transfer percentages
- [x] Concise status messages, fixed actions, scrolling content and expandable sync details
- [x] Current upload rows with safe actions to open their containing folders
- [x] Session activity for observed upload starts and confirmed sign-in/space-reclaim actions
- [x] Keyboard and accessibility activation of the Plasma tray button
- [x] Explain that hiding the SNI tray icon leaves synchronization running
- [x] Execute QML state/layout regressions in CI
- [x] Account identity, cloud quota and local free disk space from the daemon
- [x] Confirmed upload completion/error events and bounded persistent history
- [x] Complete history for namespace operations and downloads
- [x] Pause/resume with current-pass completion and continued on-demand reads
- [x] Availability jobs shared with Dolphin, with file-count progress, results and cancellation
- [x] HTTP payload byte progress and measured transfer speed, including retries and partial downloads
- [x] Actual upload queue with errors, retry delay, containing-folder actions and retry controls
- [x] Persistent unresolved sync issues, including ambiguous availability independent of queue counts
- [x] Reviewed primary-drive file conflict choices with conditional writes and Btrfs recovery
- [ ] Extend recovery beyond Btrfs and cover shared-library/deleted-remote conflicts
- [x] Combined Plasma/Dolphin setup and component diagnostics
- [x] Account/folder settings and a chooser for folders to keep offline
- [x] Per-device folder exclusions, persistent policy, Dolphin badges and fresh listing on re-inclusion
- [ ] Guided initial account/storage setup and a browser for cloud-only folder selection

The M4 UI work does not satisfy or replace M2.5's live correctness gate.
