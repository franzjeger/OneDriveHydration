# Roadmap

## M0: trustworthy skeleton

- [x] Separate repository and Rust workspace
- [x] Revision-pinned HydrationAPI dependency
- [x] GraphAccess daemon wiring
- [x] Formatting, lint, test, docs and dependency-policy CI
- [x] Publish the declared MIT OR Apache-2.0 license texts

## M1: enroll and identify the account

- [x] Device-code enrollment using the shared TokenCache
- [x] PKCE/browser enrollment threat-model review — accepted by the owner
      2026-08-13; see `docs/PKCE-ENROLLMENT-REVIEW.md` for the conditions
      that bind the implementation
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

## M3: Linux product shell

- [x] Owner-only local status and eviction CLI
- [x] D-Bus control surface
- [x] Revision-matched Linux binary payload, including privileged helper
- [x] Validated systemd installer and units
- [x] StatusNotifierItem tray icon and menu, signal-driven
- [x] Flyout: system-tray plasmoid with eviction, signal-driven
- [x] Credential state on the D-Bus surface (property + change signal), shown by
      tray and flyout, with adopt-on-restart of a fresh `pkce-enroll.py` sign-in
- [x] In-product (re-)enrollment — built under the accepted review's §7
      conditions: an `AuthCode` grant beside device code in `hydration-graph`,
      installing straight into the shared `TokenCache` with no plaintext file,
      a loopback listener bound once at literal `127.0.0.1`, surfaced as
      `auth --browser` (which also `try-restart`s a running daemon onto the
      new sign-in). The surfaces name that command; there is still
      deliberately no sign-in button, and `tools/pkce-enroll.py` remains as
      the out-of-product fallback. Outstanding from §7: the flow has not yet
      been tested against a sandboxed (Flatpak/Snap) default browser — the
      timeout diagnoses that case by name, but the measurement itself needs a
      desktop with one installed
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
