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
- [ ] D-Bus control surface
- [x] Revision-matched Linux binary payload, including privileged helper
- [ ] Validated systemd installer and units
- [ ] Tray/flyout and re-authentication UX
- [ ] Dolphin actions and status overlays
