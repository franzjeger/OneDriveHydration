//! The daemon's answer to "is this machine signed in?", served on its own
//! owner-only socket so the D-Bus service can subscribe instead of poll.
//!
//! Why a second socket rather than a new key on the control socket's `watch`
//! stream: that stream is composed inside HydrationAPI's `daemon_loop`, which
//! owns the queue, the manifest count and the exposure list — things the
//! framework knows. The credential is precisely what the framework does
//! *not* know; it lives in this product's `TokenCache`, handed to the run
//! loop only inside opaque provider roles. So the product publishes what
//! only the product can see, on `<socket>.auth` next to `daemon_loop`'s
//! `<socket>.ctl`, with the same line discipline: `status` answers once,
//! `watch` streams one `key=value` line now and one per change, and readers
//! ignore keys they do not recognise so the vocabulary can grow.
//!
//! What travels is a conclusion with exactly three values — `healthy`,
//! `unsaved`, `rejected` — because those are the three the daemon can
//! actually stand behind (measured in `tests/credential_semantics.rs`
//! against the pinned `TokenCache`):
//!
//!  * `run` refuses to start unless `resume()` loaded a credential, and
//!    nothing afterwards ever discards the loaded bytes, so in a running
//!    daemon `is_signed_in()` turning false means one thing only: the
//!    service refused the credential `MAX_REJECTIONS` times running.
//!    That is `rejected`, and it is settled — the cache has stopped
//!    spending the credential — not a retry in progress.
//!  * `last_store_error()` set means refreshes work but the rotated
//!    credential cannot be written back to Secret Service: syncing
//!    continues and the bill arrives at the next restart. `unsaved`.
//!  * Otherwise `healthy`.
//!
//! The states a *stopped* daemon would occupy — no credential stored, the
//! store locked or unreachable — are deliberately absent: they are startup
//! failures, the process that could report them has exited, and this socket
//! has died with it. A reader distinguishes those only as "the daemon is
//! not running", which is the honest ceiling; guessing further here is how
//! a UI ends up telling someone to re-enroll over a locked keyring.
//!
//! Current enrollment writes Secret Service directly and sends the owner-only
//! `enrollment-complete` command here; the publisher restarts so startup can
//! rediscover the account. While `rejected`, it also retains the alpha migration
//! path: a settled legacy `refresh-token` file triggers the same restart. This
//! module never reads credential bytes.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// The one key this socket's `watch` lines carry today. New keys may only
/// ever be appended to a line, mirroring the control socket's contract.
pub const KEY: &str = "credential";

/// The wire vocabulary, plus the reader's word for "nobody said".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CredentialState {
    /// No running daemon has asserted anything: the daemon is stopped, or it
    /// predates this socket, or it sent a value this build does not know.
    /// Never sent by the daemon — a reader-side state only. Present it as
    /// nothing: an unknown credential state is not a problem to report,
    /// and treating it as one would nag on every daemon restart.
    #[default]
    Unknown,
    /// Signed in; refreshes succeed or have not been contradicted, and the
    /// last rotation was stored.
    Healthy,
    /// Signed in and syncing, but the rotated credential could not be
    /// written to Secret Service — the next restart may cost a sign-in.
    Unsaved,
    /// The service has conclusively refused the stored credential
    /// (`MAX_REJECTIONS` consecutive `invalid_grant`s). Only a new
    /// enrollment recovers from this.
    Rejected,
}

impl CredentialState {
    /// The wire form. [`CredentialState::Unknown`] has none on purpose: the
    /// daemon never says it, so encoding it would only let a bug send it.
    pub fn as_wire(self) -> &'static str {
        match self {
            CredentialState::Unknown => "unknown",
            CredentialState::Healthy => "healthy",
            CredentialState::Unsaved => "unsaved",
            CredentialState::Rejected => "rejected",
        }
    }

    /// Values this build does not recognise become [`Unknown`] rather than
    /// an error: a newer daemon is allowed to grow the vocabulary before
    /// this reader learns to render it, and "I cannot say" is the only
    /// honest rendering of a word it cannot read.
    ///
    /// [`Unknown`]: CredentialState::Unknown
    pub fn from_wire(value: &str) -> CredentialState {
        match value {
            "healthy" => CredentialState::Healthy,
            "unsaved" => CredentialState::Unsaved,
            "rejected" => CredentialState::Rejected,
            _ => CredentialState::Unknown,
        }
    }
}

/// One sample of what the `TokenCache` mirrors out: the two facts the
/// conclusion is folded from. Sampled in-process — the cache's accessors
/// read a mirror and never block on the refresh lock, so a one-second
/// cadence costs nothing anyone can observe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialHealth {
    /// `TokenCache::is_signed_in()`.
    pub signed_in: bool,
    /// `TokenCache::last_store_error()`.
    pub store_error: Option<io::ErrorKind>,
}

impl CredentialHealth {
    /// Fold the facts into the one conclusion the wire carries. Rejection
    /// outranks the store: once the service has refused the credential,
    /// whether its rotations were being saved is of no further interest.
    pub fn state(self) -> CredentialState {
        if !self.signed_in {
            CredentialState::Rejected
        } else if self.store_error.is_some() {
            CredentialState::Unsaved
        } else {
            CredentialState::Healthy
        }
    }
}

/// Where the auth-state socket lives, given the daemon's `--socket` path:
/// the extension becomes `auth`, exactly as `daemon_loop` derives `.ctl`.
pub fn auth_socket(main_socket: &Path) -> PathBuf {
    main_socket.with_extension("auth")
}

/// Tell a running daemon that a new credential was stored directly in Secret
/// Service. The daemon exits nonzero so its `Restart=on-failure` unit reloads
/// the credential and rediscovers the account before syncing another byte.
pub fn notify_enrollment(main_socket: &Path) -> io::Result<()> {
    let reply = crate::control_request(&auth_socket(main_socket), "enrollment-complete")?;
    if reply == "restarting to adopt the new sign-in" {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the daemon returned an unexpected enrollment reply",
        ))
    }
}

/// More live watchers than this and new ones are refused by closing them
/// without a line — the same refusal shape as the control socket, and
/// handled by the same reconnect-with-backoff on the other side. The
/// legitimate population is one D-Bus service and the occasional `ctl`;
/// eight is already generous.
const MAX_WATCHERS: usize = 8;

/// How often the publisher looks at the cache and, while rejected, at the
/// enrollment file. In-process reads of a lock-free mirror; the cadence is
/// the control socket's status-thread cadence, not a network poll.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Whether the peer end of this stream has gone away, without reading it.
///
/// A zero-timeout poll for no events: `HUP`, `ERR` and `NVAL` are reported
/// whether or not they were requested, and they are the only three of
/// interest — asking for readability would confuse an unread buffer with a
/// departure. Without this cull a watcher that disconnects during a quiet
/// stretch holds its slot until the next state change, and credential state
/// changes are rare by design; a reconnecting reader could then fill the
/// registry with corpses and be refused its live connection.
fn peer_gone(conn: &UnixStream) -> bool {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    let mut fds = [PollFd::new(conn, PollFlags::empty())];
    match poll(&mut fds, Some(&Timespec::default())) {
        Ok(n) if n > 0 => fds[0]
            .revents()
            .intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL),
        _ => false,
    }
}

fn line(state: CredentialState) -> String {
    format!("{KEY}={}", state.as_wire())
}

/// The `status` verb's one-line answer: the conclusion plus, for `unsaved`,
/// the stored `io::ErrorKind` — the only detail the cache keeps, kept here
/// for the same reason (a store error message is a place credential bytes
/// could travel; a kind is not).
fn status_line(health: CredentialHealth) -> String {
    match health.state() {
        // Unreachable from a daemon (see `CredentialState::Unknown`), kept
        // total so a future refactor cannot make it silently reachable.
        CredentialState::Unknown => "sign-in: unknown".to_owned(),
        CredentialState::Healthy => {
            "sign-in: healthy — the stored sign-in works and rotations are being saved".to_owned()
        }
        CredentialState::Unsaved => format!(
            "sign-in: working, but the rotated sign-in could not be saved to Linux Secret \
             Service ({:?}); unlock the keyring (ksecretd or gnome-keyring) or the next \
             daemon start may require signing in again",
            health.store_error.unwrap_or(io::ErrorKind::Other)
        ),
        CredentialState::Rejected => "sign-in: REQUIRED — OneDrive no longer accepts this \
             machine's saved sign-in. Use the flyout's Sign in button or run \
             onedrive-hydration-daemon reauth; the daemon restarts onto the new sign-in."
            .to_owned(),
    }
}

/// Everything the publisher needs beyond the cache itself.
pub struct PublisherOptions {
    /// See [`SAMPLE_INTERVAL`]; tests shorten it so they assert on events
    /// rather than living through seconds.
    pub sample_interval: Duration,
    /// Where a fresh enrollment appears (`<state-dir>/refresh-token`), or
    /// `None` to disable adoption entirely.
    pub enrollment: Option<PathBuf>,
}

/// Serve the auth-state socket until `keep_going` says stop.
///
/// Binds `socket` owner-only, answers `status` and `watch` on an acceptor
/// thread, and runs the sample/broadcast loop on the calling thread. The
/// seams are injected the way `dbus::watch_daemon`'s are, and for the same
/// reason: `sample` so tests script the cache instead of needing a live
/// credential (the repository rule), `adopt` so "restart the daemon" is a
/// recorded call in tests and `std::process::exit(1)` in the binary,
/// `keep_going` so tests bound the loop without real time.
///
/// The acceptor thread is not joined: it blocks in `accept` and lives for
/// the process, exactly like `daemon_loop`'s control thread. Tests leak one
/// per publisher, bound to a socket in a directory that dies with them.
pub fn serve(
    socket: &Path,
    options: PublisherOptions,
    sample: &mut dyn FnMut() -> CredentialHealth,
    adopt: &mut dyn FnMut(),
    keep_going: &mut dyn FnMut() -> bool,
) -> io::Result<()> {
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    // Owner-only, like the control socket: the state is the owner's to read
    // and nobody else's business.
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;

    // Sampled before the acceptor exists so the first connection ever
    // adopted is told a real state, not a default.
    let mut health = sample();
    let current = Arc::new(Mutex::new(health));
    let watchers: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
    let restart_requested = Arc::new(AtomicBool::new(false));

    {
        let (current, watchers, restart_requested) = (
            Arc::clone(&current),
            Arc::clone(&watchers),
            Arc::clone(&restart_requested),
        );
        std::thread::spawn(move || accept_loop(listener, &current, &watchers, &restart_requested));
    }

    // The enrollment file must hold still for one whole interval before it
    // is believed: the enrollment tool writes it in one go, but "one go"
    // is a claim about a Python process this daemon cannot see, and
    // restarting onto a half-written credential would spend the user's
    // enrollment on a parse failure. Two identical size/mtime snapshots in
    // a row are the settle test.
    let mut pending: Option<(u64, SystemTime)> = None;
    // Which settled file was already adopted, so a test's `adopt` that
    // returns (the binary's never does) is not called again for the same
    // bytes on every subsequent tick.
    let mut adopted: Option<(u64, SystemTime)> = None;
    // An enrollment noticed while the sign-in still works is announced once,
    // not once per second.
    let mut announced_waiting_enrollment = false;

    while keep_going() {
        let sampled = sample();
        if sampled != health {
            let previous = health.state();
            health = sampled;
            *current.lock().unwrap() = health;
            let state = health.state();
            if state != previous {
                // The journal is where someone debugging will look; the
                // transition and what to do about it belong there too.
                eprintln!(
                    "onedrive-hydration-daemon: sign-in state {} -> {} ({})",
                    previous.as_wire(),
                    state.as_wire(),
                    status_line(health)
                );
                broadcast(&watchers, &line(state));
            }
        }
        cull(&watchers);

        // Browser enrollment writes directly to Secret Service. Restart even
        // when the old credential is still healthy: the new sign-in may name a
        // different account, and drive discovery must run before it is used.
        if restart_requested.swap(false, Ordering::SeqCst) {
            eprintln!(
                "onedrive-hydration-daemon: a new sign-in was stored in Secret Service; \
                 restarting to reload it and rediscover the account"
            );
            adopt();
        }

        if let Some(path) = options.enrollment.as_deref() {
            let snapshot = crate::enrollment_snapshot(path);
            match (health.state(), snapshot) {
                (CredentialState::Rejected, Some(snap)) => {
                    if adopted == Some(snap) {
                        // Already handed to `adopt`; nothing new to do.
                    } else if pending == Some(snap) {
                        eprintln!(
                            "onedrive-hydration-daemon: a fresh enrollment appeared at {}; \
                             restarting to adopt it (the startup path moves it into Secret \
                             Service and signs in with it)",
                            path.display()
                        );
                        adopted = Some(snap);
                        adopt();
                    } else {
                        pending = Some(snap);
                    }
                }
                (_, Some(_)) => {
                    // Signed in and an enrollment file exists: somebody
                    // enrolled deliberately while the current sign-in still
                    // works. Adopting it now would silently switch accounts
                    // under a running sync; the startup path adopts it at
                    // the next start, where drive discovery re-runs. Say so
                    // once rather than every second.
                    if !announced_waiting_enrollment {
                        eprintln!(
                            "onedrive-hydration-daemon: an enrollment file exists at {} but \
                             the current sign-in still works; it will be adopted at the next \
                             daemon start",
                            path.display()
                        );
                        announced_waiting_enrollment = true;
                    }
                    pending = None;
                }
                (_, None) => {
                    pending = None;
                    announced_waiting_enrollment = false;
                }
            }
        }

        std::thread::sleep(options.sample_interval);
    }
    Ok(())
}

/// One connection at a time on the accept thread, like the control socket:
/// everything answered here is a line or an adoption, and a peer that
/// connects and never speaks is bounded by the read timeout rather than
/// parking the channel forever.
fn accept_loop(
    listener: UnixListener,
    current: &Mutex<CredentialHealth>,
    watchers: &Mutex<Vec<UnixStream>>,
    restart_requested: &AtomicBool,
) {
    for conn in listener.incoming().flatten() {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
        let reader = BufReader::new(match conn.try_clone() {
            Ok(c) => c,
            Err(_) => continue,
        });
        let mut out = conn;
        for l in reader.lines().map_while(Result::ok) {
            let verb = l.trim();
            let reply = match verb {
                "status" => status_line(*current.lock().unwrap()),
                "watch" => {
                    // From here the connection is written to and never read;
                    // it joins the registry the sample loop broadcasts to.
                    // Over the cap it is dropped without a line — the same
                    // bare-EOF refusal the control socket uses, which the
                    // reader side already treats as "retry with backoff".
                    let _ = out.set_read_timeout(None);
                    let mut conns = watchers.lock().unwrap();
                    if conns.len() < MAX_WATCHERS {
                        let state = current.lock().unwrap().state();
                        if writeln!(out, "{}", line(state)).is_ok() {
                            conns.push(out);
                        }
                    }
                    break;
                }
                "enrollment-complete" => {
                    restart_requested.store(true, Ordering::SeqCst);
                    "restarting to adopt the new sign-in".to_owned()
                }
                "" => continue,
                other => format!("unknown command: {other}"),
            };
            if writeln!(out, "{reply}").is_err() {
                break;
            }
        }
    }
}

/// Write one line to every watcher, dropping the ones that fail. Rust
/// ignores `SIGPIPE` at startup, so a dead peer surfaces as `EPIPE` here
/// rather than as a signal.
fn broadcast(watchers: &Mutex<Vec<UnixStream>>, line: &str) {
    watchers
        .lock()
        .unwrap()
        .retain_mut(|conn| writeln!(conn, "{line}").is_ok());
}

/// Drop watchers whose peers have hung up. Broadcasts already prune on
/// write failure; this catches departures during the quiet stretches that
/// are this socket's normal condition.
fn cull(watchers: &Mutex<Vec<UnixStream>>) {
    watchers.lock().unwrap().retain(|conn| !peer_gone(conn));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_conclusion_folds_the_two_facts_with_rejection_first() {
        let healthy = CredentialHealth {
            signed_in: true,
            store_error: None,
        };
        assert_eq!(healthy.state(), CredentialState::Healthy);

        let unsaved = CredentialHealth {
            signed_in: true,
            store_error: Some(io::ErrorKind::PermissionDenied),
        };
        assert_eq!(unsaved.state(), CredentialState::Unsaved);

        // A store error alongside a rejection changes nothing: the
        // credential is dead either way, and "rejected" is the message
        // with an action attached.
        let rejected = CredentialHealth {
            signed_in: false,
            store_error: Some(io::ErrorKind::PermissionDenied),
        };
        assert_eq!(rejected.state(), CredentialState::Rejected);
    }

    #[test]
    fn the_wire_vocabulary_round_trips_and_unknown_words_degrade() {
        for state in [
            CredentialState::Healthy,
            CredentialState::Unsaved,
            CredentialState::Rejected,
        ] {
            assert_eq!(CredentialState::from_wire(state.as_wire()), state);
        }
        // A newer daemon's new word must not break an older reader.
        assert_eq!(
            CredentialState::from_wire("quarantined"),
            CredentialState::Unknown
        );
        assert_eq!(CredentialState::from_wire(""), CredentialState::Unknown);
    }

    #[test]
    fn the_auth_socket_sits_next_to_the_control_socket() {
        assert_eq!(
            auth_socket(Path::new("/run/user/1000/onedrive-hydration.sock")),
            Path::new("/run/user/1000/onedrive-hydration.auth")
        );
        // The daemon derives it from its `--socket` (`.sock`); the D-Bus
        // service and the ctl derive it from the *control* socket (`.ctl`),
        // which daemon_loop in turn derives from the same `--socket`. The
        // two derivations must land on one path or the surface would watch
        // a socket nobody serves.
        assert_eq!(
            auth_socket(Path::new("/run/user/1000/onedrive-hydration.sock")),
            auth_socket(Path::new("/run/user/1000/onedrive-hydration.ctl")),
        );
    }

    #[test]
    fn the_status_line_names_the_action_only_when_there_is_one() {
        let healthy = status_line(CredentialHealth {
            signed_in: true,
            store_error: None,
        });
        assert!(healthy.contains("healthy"), "{healthy}");
        assert!(!healthy.contains("pkce-enroll"), "{healthy}");

        let unsaved = status_line(CredentialHealth {
            signed_in: true,
            store_error: Some(io::ErrorKind::PermissionDenied),
        });
        assert!(unsaved.contains("PermissionDenied"), "{unsaved}");
        assert!(unsaved.contains("unlock the keyring"), "{unsaved}");

        let rejected = status_line(CredentialHealth {
            signed_in: false,
            store_error: None,
        });
        assert!(
            rejected.contains("onedrive-hydration-daemon reauth"),
            "{rejected}"
        );
        assert!(rejected.contains("restarts onto"), "{rejected}");
    }
}
