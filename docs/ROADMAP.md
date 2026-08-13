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

## M3: Linux product shell

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
