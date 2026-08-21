# Findings from the live rig (KDE Plasma 6.7.4, 165 424 files)

Measured on the production mount, 2026-08-16. House style: every claim is
cited to `file:line` or to a measurement, and the obvious-but-wrong reading
is named where it exists.

## What now works

- **Per-file emblems.** The overlay plugin
  (`packaging/dolphin/overlay/hydrationoverlay.cpp`) reads
  `user.hydration.dehydrated`; cloud-only → `cloud-download` emblem,
  resident → `emblem-success`.
- **Folder emblems.** `probeDirectory()` is a bounded BFS (depth ≤ 4, ≤ 30
  files, early-exit on the first dehydrated file) instead of a depth-1 scan —
  a folder whose dehydrated files sit in subdirectories now badges. Verified:
  a 130/130-dehydrated folder shows cloud, a 0/88 folder shows the check, an
  empty folder shows nothing. Fifteen of the seventeen folders that previously
  showed no badge are genuinely empty — the old code was right for them.
- **Emblem refresh after eviction.** `onFilesChanged` fires, and
  `KFileItemModelRolesUpdater::slotOverlaysChanged` in
  `libdolphinprivate.so` consumes `overlaysChanged`. Eviction changes mtime
  (the old code comment claiming otherwise is wrong), so the refresh path
  needs no special handling.
- **Servicemenu placement.** KIO puts priority-less services into the
  "Actions" submenu (`kio/src/widgets/kfileitemactions.cpp:536-556`);
  `X-KDE-Priority=TopLevel` lifts them to top level. Both file and folder
  entries carry it now.
- **"Free Up Space" on pinned folders.** `free-up-space-folder.sh` runs
  `unpin` before the per-file evict loop, so a pinned folder can actually be
  evicted. Verified end-to-end: re-hydrate → wrapper → blocks 32→0,
  `user.hydration.dehydrated` set.
- **Progress box: speed + ETA.** `keep-on-device.sh` now renders
  `Downloading N of T — name · R files/s · ~ETA left`, computed from the
  wall clock (`date +%s`, whole seconds — the per-file latency dwarfs the
  remainder).

## Measured performance floor

- `onedrive-hydrationctl status` (a trivial control-socket call): **~865 ms**.
- `hydrate` of a **0-byte** placeholder: **~300 ms** (359/292/315 ms over
  three runs). A bare `open+read` of the same 0-byte file from C:
  **~300 ms** — the cost is in the framework's round trip, not in the shell
  or the Rust spawn (`ctl` with no arguments: 0 ms).
- 39 KB file: **~609 ms** — the same ~300 ms base plus the fetch.

So the per-file floor is ~300 ms, and it is **serial**: HydrationAPI runs one
fetch thread on one socket with one request outstanding
(`HydrationAPI/crates/hydrationd/src/daemon.rs:401` — one fetch thread
serving a single request queue).
For the 17 224 pending files under `Projects/FullAudit2/.venv`:
**~87 minutes**, and no wrapper-level batching removes it — the latency is
per network round trip, not per process.

## Behaviours worth knowing

- **`hydrate` takes an absolute path; `evict`/`pin`/`unpin`/`pending` take
  paths relative to the sync root.** Mixing them up fails with a path error.
- **EIO while the daemon is busy.** `hydrate` of a file in a *different*
  folder returns `error: Input/output error (os error 5)` while another pull
  is in flight — the daemon is single-flight and the socket answers EIO
  rather than queueing. A wrapper that sees this should retry, not report it
  as a file fault. (Observed live: two FullAudit2 pulls in flight, a
  third-folder hydrate failed with EIO, and succeeded once the daemon was
  idle.)
- **Dolphin spawns wrappers with a minimal environment.**
  `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS` must be exported in the
  wrapper or `kdialog`/`qdbus`/`dbus-send` silently do nothing.
- **The progress box is `kdialog --progressbar`** (a KDE `QProgressDialog`
  driven over D-Bus), not the plasmoid. The plasmoid only sees the daemon's
  `Downloading` property, which is structurally 0/1 — the "N of T" batch
  state lives in the shell loop and is invisible to the tray.

## Remaining ideas (in rough order of cost)

1. **Completed: bounded retry on measured busy EIO.** Keep on Device retries
   only the exact `error: Input/output error (os error 5)` result five times with a
   one-second backoff; other errors remain immediate.
2. **Batch progress on the tray/flyout** (architectural): the wrapper would
   push `total/done/bytes` to the daemon over a new D-Bus method, and the
   plasmoid would render it — unifying the box and the tray. Blocked on a
   daemon-side method; the D-Bus surface today is
   `DaemonRunning`, `Unsent`, `Excluded`, `Exposures`, `CredentialState`,
   `Downloading`, `Indexing`, `Uploading`, enrollment, plus `Evict(s)`.
3. **Quota / storage used in the flyout** (daemon change): not in the D-Bus
   surface; needs the daemon to expose it.
4. **Pause/resume sync** (daemon change): not implemented anywhere.
5. **Per-file byte-level progress** (daemon change): the daemon exposes the
   *count* of downloads in flight, not bytes transferred/total.
6. **Parallel hydration** (security-relevant framework change, HydrationAPI):
   request pipelining via the protocol `id` field + a fetch-thread pool +
   daemon-side concurrency. This is the only thing that breaks the ~300 ms
   serial floor. It weakens the single-flight property that keeps two files
   from substituting each other's content, so it needs a threat-model pass,
   not a config toggle. See the HydrationAPI repo for the seam.
