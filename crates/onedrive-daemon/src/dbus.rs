//! The session-bus face of the daemon, so a tray never polls and never learns
//! what a unix socket is.
//!
//! The daemon's control socket stays the single authority: this surface holds
//! no state of its own beyond a cache of the last state line it saw, and every
//! eviction is one `evict` request over the same socket the CLI uses. The
//! service therefore adds no second owner of queue or placeholder truth — it
//! is a translator, and when it disagrees with the socket, the socket is
//! right.
//!
//! It lives on the *session* bus only. The daemon is a user process and its
//! socket is owner-only (`0o600`); the session bus enforces the same boundary
//! at AUTH time, because only the session owner can complete `EXTERNAL`
//! authentication against it. Eviction additionally re-checks the caller's
//! uid through the bus driver (see `ControlSurface::caller_permitted`) so
//! that running against a misconfigured or shared bus does not quietly widen
//! who may throw away local bytes.
//!
//! State flows one way. The service holds a single long-lived `watch`
//! connection to the daemon (one state line immediately, another per change,
//! no line when nothing changed, nothing else on that connection ever) and
//! republishes each line as D-Bus property values plus a `StateChanged`
//! signal. However many trays and flyouts subscribe here, the daemon sees
//! exactly one watcher — this service — out of the small number it caps
//! watchers at; the connection is held even while nobody is subscribed,
//! because the properties have to answer a cold read correctly either way.
//!
//! When the daemon restarts, its socket is unlinked and rebound, so the held
//! connection dies; [`watch_daemon`] reconnects with exponential backoff
//! between [`RETRY_FLOOR`] and [`RETRY_CEILING`] rather than spinning, and
//! flips `DaemonRunning` to false in between so a tray shows "not running"
//! instead of stale numbers. A daemon at its watcher cap refuses by closing
//! the connection without a line — indistinguishable from a restart mid
//! connect, and handled the same way: the bounded retry, not an error.

use crate::control_request;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;

/// The well-known bus name the service owns, the object path it serves, and
/// the interface it exposes there. All three reuse the identifier the daemon
/// already registers with Secret Service, so the product presents one name
/// everywhere a user might look for it.
pub const BUS_NAME: &str = "io.github.franzjeger.OneDriveHydration";
/// See [`BUS_NAME`].
pub const OBJECT_PATH: &str = "/io/github/franzjeger/OneDriveHydration";
/// See [`BUS_NAME`].
pub const INTERFACE: &str = "io.github.franzjeger.OneDriveHydration";

/// Delay before the first reconnect attempt, and the value the delay resets
/// to after a connection that produced at least one state line.
pub const RETRY_FLOOR: Duration = Duration::from_secs(1);
/// The delay stops doubling here. A daemon that stays away costs one failed
/// `connect()` every thirty seconds, which is bounded and invisible; a tray
/// that waited minutes to notice a restarted daemon would not be.
pub const RETRY_CEILING: Duration = Duration::from_secs(30);

/// What the tray gets to know, exactly as the daemon last told us.
///
/// The counters keep their last-seen values while `daemon_running` is false.
/// That is deliberate: a tray decides "not running" from the flag alone, and
/// zeroing the counters would manufacture a state line the daemon never sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DaemonState {
    /// The control socket currently accepts and answers.
    pub daemon_running: bool,
    /// Local changes not yet uploaded, including uploads in flight.
    pub unsent: u64,
    /// Files excluded from backup because they are placeholders.
    pub excluded: u64,
    /// Other mounts that expose the sync root's files without hydration.
    pub exposures: u64,
}

/// Fold one `watch` state line into `state`. Returns whether the line carried
/// at least one key this build recognises.
///
/// A state line is space-separated `key=value`. Keys this build does not know
/// are skipped rather than refused — the daemon is allowed to grow new fields
/// before the tray learns to render them, and a surface that breaks on
/// upgrade is worse than one that shows a subset. A value that does not parse
/// as `u64` is skipped the same way, keeping the previous value, because a
/// half-applied line is still better than a dead watch connection.
pub fn apply_state_line(line: &str, state: &mut DaemonState) -> bool {
    let mut recognized = false;
    for token in line.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        match key {
            "unsent" => state.unsent = value,
            "excluded" => state.excluded = value,
            "exposures" => state.exposures = value,
            _ => continue,
        }
        recognized = true;
    }
    recognized
}

/// One `watch` connection: publish every state line until the daemon hangs
/// up. Returns whether any state line arrived, which is what [`watch_daemon`]
/// uses to decide the connection was real enough to reset its backoff.
///
/// "Running" is decided by the first state line, not by the connect. The
/// daemon writes that line synchronously while adopting a watcher, so waiting
/// for it costs nothing — and the daemon refuses watchers over its cap by
/// closing the connection with a bare EOF and no line at all, which a
/// connect-time flag would misreport as one "running"/"stopped" flap per
/// retry, forever.
///
/// No read timeout is set. Silence is the healthy condition here — the daemon
/// promises a line only when something changed — so a timeout would turn a
/// quiet afternoon into a reconnect loop. Death of the daemon still surfaces
/// promptly as EOF because the socket closes with its process. The write half
/// stays open too, unlike `control_request`'s: after `watch` the daemon never
/// reads again, and the fully open socket is the whole subscription.
fn watch_once(
    socket: &Path,
    state: &mut DaemonState,
    on_state: &mut dyn FnMut(DaemonState),
) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    if stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .is_err()
        || stream.write_all(b"watch\n").is_err()
    {
        return false;
    }
    let mut saw_state_line = false;
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if apply_state_line(&line, state) {
            state.daemon_running = true;
            saw_state_line = true;
            on_state(*state);
        }
    }
    saw_state_line
}

/// Hold a `watch` connection to the daemon forever, reconnecting when it
/// drops, and hand every *distinct* state to `publish`.
///
/// Deduplication happens here even though the daemon already suppresses
/// identical consecutive lines, because a reconnect legitimately replays the
/// current state and the flag flips are ours, not the daemon's. `publish` is
/// assumed to start from [`DaemonState::default`] — the same state a freshly
/// served [`ControlSurface`] exposes — so a daemon that is simply absent
/// produces no publishes at all.
///
/// `sleep` and `keep_going` exist so tests can run the loop against a
/// scripted socket without real time passing; the binary passes
/// `thread::sleep` and `|| true`.
pub fn watch_daemon(
    socket: &Path,
    publish: &mut dyn FnMut(DaemonState),
    sleep: &mut dyn FnMut(Duration),
    keep_going: &mut dyn FnMut() -> bool,
) {
    let mut state = DaemonState::default();
    let mut last_published = DaemonState::default();
    let mut delay = RETRY_FLOOR;
    while keep_going() {
        let mut publish_distinct = |s: DaemonState| {
            if s != last_published {
                last_published = s;
                publish(s);
            }
        };
        let saw_state_line = watch_once(socket, &mut state, &mut publish_distinct);
        state.daemon_running = false;
        publish_distinct(state);
        // Reset only when the daemon actually spoke the protocol. A socket
        // that accepts but never sends a state line — a daemon at its
        // watcher cap refusing with a bare EOF, or one wedged at startup —
        // keeps the doubling, so retrying against a full daemon costs one
        // connect per ceiling interval, not a loop.
        if saw_state_line {
            delay = RETRY_FLOOR;
            sleep(delay);
        } else {
            sleep(delay);
            delay = RETRY_CEILING.min(delay * 2);
        }
    }
}

/// The daemon's one-line answer to `evict`, in a shape a caller can act on.
#[derive(Debug, PartialEq, Eq)]
pub enum EvictReply {
    /// `reclaimed <n> bytes` — the file is a placeholder again.
    Reclaimed(u64),
    /// `kept: <reason>` — the daemon refused, and the file is untouched.
    Kept(String),
    /// `error: <e>`, or a reply this build does not recognise, passed through
    /// verbatim so the user sees what the daemon actually said rather than a
    /// summary invented here.
    Failed(String),
}

/// Parse the control socket's reply to `evict`.
pub fn parse_evict_reply(reply: &str) -> EvictReply {
    if let Some(bytes) = reply
        .strip_prefix("reclaimed ")
        .and_then(|r| r.strip_suffix(" bytes"))
        .and_then(|n| n.parse().ok())
    {
        return EvictReply::Reclaimed(bytes);
    }
    if let Some(reason) = reply.strip_prefix("kept: ") {
        return EvictReply::Kept(reason.to_owned());
    }
    if let Some(error) = reply.strip_prefix("error: ") {
        return EvictReply::Failed(error.to_owned());
    }
    EvictReply::Failed(format!("unrecognized daemon reply: {reply}"))
}

/// Errors `Evict` can return, named so a tray can branch on them.
///
/// `Kept` is deliberately an error and not a zero-byte success: the user
/// asked for space back and did not get it, and the two callers written so
/// far both wanted to say why.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "io.github.franzjeger.OneDriveHydration.Error")]
pub enum Error {
    /// A transport-level zbus failure, kept for completeness.
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The control socket is absent or refused the connection — the daemon
    /// is not running. A tray shows "not running", not a stack trace.
    DaemonUnavailable(String),
    /// The daemon refused the eviction and said why (open file, unsent
    /// content, not a hydrated placeholder, ...).
    Kept(String),
    /// The daemon answered `error:`, answered something unrecognisable, or
    /// the request could not be completed.
    Failed(String),
    /// The path was empty or contained bytes the line protocol cannot carry.
    InvalidPath(String),
    /// The caller's identity could not be established or is not the owner.
    Denied(String),
}

/// The object served at [`OBJECT_PATH`].
///
/// Reads are answered from the cached [`DaemonState`]; only `Evict` touches
/// the daemon, over a fresh short-lived control connection so a stuck
/// eviction can never wedge the state feed.
pub struct ControlSurface {
    socket: PathBuf,
    state: DaemonState,
    /// When `Some(uid)`, `Evict` refuses callers the bus cannot attribute to
    /// exactly that uid. The binary always sets this to its own euid,
    /// mirroring the socket's `0o600`. `None` exists for the peer-to-peer
    /// test harness only, where there is no bus driver to ask and the
    /// transport is a socketpair nothing else can reach.
    require_uid: Option<u32>,
}

impl ControlSurface {
    pub fn new(socket: PathBuf, require_uid: Option<u32>) -> Self {
        Self {
            socket,
            state: DaemonState::default(),
            require_uid,
        }
    }

    /// The eviction gate. The session bus already refuses other users at
    /// AUTH, so on a healthy system this is redundant — but "the bus is
    /// configured the way we assume" is exactly the kind of claim this
    /// project does not build destructive operations on. The check asks the
    /// bus driver who the caller is and fails closed: no sender, no answer,
    /// or no uid in the answer all deny.
    async fn caller_permitted(
        &self,
        header: &Header<'_>,
        connection: &zbus::Connection,
    ) -> Result<(), Error> {
        let Some(required) = self.require_uid else {
            return Ok(());
        };
        let Some(sender) = header.sender() else {
            return Err(Error::Denied(
                "the caller has no bus identity to check".to_owned(),
            ));
        };
        let credentials = zbus::fdo::DBusProxy::new(connection)
            .await
            .map_err(|e| Error::Denied(format!("could not reach the bus driver: {e}")))?
            .get_connection_credentials(BusName::Unique(sender.clone()))
            .await
            .map_err(|e| Error::Denied(format!("the bus could not identify the caller: {e}")))?;
        match credentials.unix_user_id() {
            Some(uid) if uid == required => Ok(()),
            Some(uid) => Err(Error::Denied(format!(
                "eviction is owner-only: caller has uid {uid}, the daemon owner is {required}"
            ))),
            None => Err(Error::Denied(
                "the bus did not report a uid for the caller".to_owned(),
            )),
        }
    }
}

#[zbus::interface(name = "io.github.franzjeger.OneDriveHydration")]
impl ControlSurface {
    /// Whether the daemon's control socket currently answers. When this is
    /// false the counter properties hold their last-seen values.
    #[zbus(property)]
    fn daemon_running(&self) -> bool {
        self.state.daemon_running
    }

    /// Local changes not yet uploaded, including uploads in flight.
    #[zbus(property)]
    fn unsent(&self) -> u64 {
        self.state.unsent
    }

    /// Files excluded from backup because they are placeholders.
    #[zbus(property)]
    fn excluded(&self) -> u64 {
        self.state.excluded
    }

    /// Other mounts that expose the sync root's files without hydration.
    /// Anything above zero is a warning state.
    #[zbus(property)]
    fn exposures(&self) -> u64 {
        self.state.exposures
    }

    /// Return a hydrated file to a placeholder, freeing its local bytes.
    ///
    /// `path` is relative to the sync root and goes to the daemon unchanged —
    /// the daemon's reclaim path is the only place that decides what a path
    /// means, and it already refuses escapes. Returns the number of bytes
    /// reclaimed, or one of the named errors in [`Error`].
    async fn evict(
        &self,
        path: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<u64, Error> {
        self.caller_permitted(&header, connection).await?;
        if path.is_empty() || path.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
            return Err(Error::InvalidPath(
                "eviction needs a non-empty path without newline or NUL bytes".to_owned(),
            ));
        }
        let socket = self.socket.clone();
        let command = format!("evict {path}");
        // On a worker thread because `control_request` blocks for up to ten
        // seconds, and blocking here would stall every property read and
        // signal on this connection for the duration.
        match blocking::unblock(move || control_request(&socket, &command)).await {
            Ok(reply) => match parse_evict_reply(&reply) {
                EvictReply::Reclaimed(bytes) => Ok(bytes),
                EvictReply::Kept(reason) => Err(Error::Kept(reason)),
                EvictReply::Failed(detail) => Err(Error::Failed(detail)),
            },
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                Err(Error::DaemonUnavailable(format!(
                    "the daemon control socket did not answer: {e}"
                )))
            }
            Err(e) => Err(Error::Failed(format!("control request failed: {e}"))),
        }
    }

    /// Fired once per distinct state, carrying the same values as the
    /// properties so a subscriber never needs a follow-up read. Identical
    /// consecutive states are not signalled.
    #[zbus(signal)]
    pub async fn state_changed(
        emitter: &SignalEmitter<'_>,
        daemon_running: bool,
        unsent: u64,
        excluded: u64,
        exposures: u64,
    ) -> zbus::Result<()>;
}

/// Push a new state into a served [`ControlSurface`]: update the properties,
/// emit `PropertiesChanged` for each one that moved, then `StateChanged`.
///
/// Callable from a plain thread — this is the bridge [`watch_daemon`]'s
/// `publish` closure is expected to be built from. The caller is responsible
/// for only passing distinct states; this function emits unconditionally.
pub fn publish_state(
    iface: &zbus::blocking::object_server::InterfaceRef<ControlSurface>,
    state: DaemonState,
) -> zbus::Result<()> {
    let mut surface = iface.get_mut();
    let previous = surface.state;
    surface.state = state;
    let emitter = iface.signal_emitter();
    zbus::block_on(async {
        if previous.daemon_running != state.daemon_running {
            surface.daemon_running_changed(emitter).await?;
        }
        if previous.unsent != state.unsent {
            surface.unsent_changed(emitter).await?;
        }
        if previous.excluded != state.excluded {
            surface.excluded_changed(emitter).await?;
        }
        if previous.exposures != state.exposures {
            surface.exposures_changed(emitter).await?;
        }
        ControlSurface::state_changed(
            emitter,
            state.daemon_running,
            state.unsent,
            state.excluded,
            state.exposures,
        )
        .await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    #[test]
    fn state_line_updates_only_the_keys_it_names() {
        let mut state = DaemonState::default();
        assert!(apply_state_line(
            "unsent=3 excluded=10 exposures=1",
            &mut state
        ));
        assert_eq!(
            state,
            DaemonState {
                daemon_running: false,
                unsent: 3,
                excluded: 10,
                exposures: 1,
            }
        );
        // A later line may carry fewer keys; the missing ones keep their
        // values rather than resetting.
        assert!(apply_state_line("unsent=2", &mut state));
        assert_eq!(state.unsent, 2);
        assert_eq!(state.excluded, 10);
    }

    #[test]
    fn unknown_keys_and_malformed_values_are_skipped_not_fatal() {
        let mut state = DaemonState::default();
        assert!(apply_state_line(
            "unsent=1 shiny_new_field=yes conflicts=4 excluded=2 exposures=0",
            &mut state
        ));
        assert_eq!((state.unsent, state.excluded, state.exposures), (1, 2, 0));
        // A known key with an unparseable value keeps the previous value.
        assert!(apply_state_line("unsent=lots excluded=3", &mut state));
        assert_eq!((state.unsent, state.excluded), (1, 3));
    }

    #[test]
    fn a_line_with_no_recognized_keys_is_not_a_state_line() {
        let mut state = DaemonState::default();
        assert!(!apply_state_line("unknown command: watch", &mut state));
        assert!(!apply_state_line("", &mut state));
        assert_eq!(state, DaemonState::default());
    }

    #[test]
    fn evict_replies_parse_into_the_three_outcomes() {
        assert_eq!(
            parse_evict_reply("reclaimed 4096 bytes"),
            EvictReply::Reclaimed(4096)
        );
        assert_eq!(
            parse_evict_reply("kept: OpenByAnotherProcess"),
            EvictReply::Kept("OpenByAnotherProcess".to_owned())
        );
        assert_eq!(
            parse_evict_reply("error: no such placeholder"),
            EvictReply::Failed("no such placeholder".to_owned())
        );
        assert_eq!(
            parse_evict_reply("unknown command: evict x"),
            EvictReply::Failed("unrecognized daemon reply: unknown command: evict x".to_owned())
        );
    }

    /// The full life of a watch: connect, states, daemon restart, reconnect.
    #[test]
    fn watch_publishes_distinct_states_across_a_daemon_restart() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.ctl");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            // First daemon lifetime.
            let (mut conn, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(conn.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line, "watch\n");
            conn.write_all(b"unsent=3 excluded=10 exposures=0\n")
                .unwrap();
            // The daemon promises not to repeat itself, but the client must
            // not depend on that promise.
            conn.write_all(b"unsent=3 excluded=10 exposures=0\n")
                .unwrap();
            conn.write_all(b"unsent=2 excluded=10 exposures=0 future=9\n")
                .unwrap();
            drop(conn);
            // The daemon restarted: a fresh connection replays current state.
            let (mut conn, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(conn.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line, "watch\n");
            conn.write_all(b"unsent=2 excluded=10 exposures=0\n")
                .unwrap();
            conn.write_all(b"unsent=0 excluded=12 exposures=1\n")
                .unwrap();
        });

        let mut published = Vec::new();
        let mut sleeps = Vec::new();
        let mut rounds = 0;
        watch_daemon(
            &socket,
            &mut |s| published.push(s),
            &mut |d| sleeps.push(d),
            &mut || {
                rounds += 1;
                rounds <= 2
            },
        );
        server.join().unwrap();

        let s = |daemon_running, unsent, excluded, exposures| DaemonState {
            daemon_running,
            unsent,
            excluded,
            exposures,
        };
        assert_eq!(
            published,
            [
                s(true, 3, 10, 0),  // first state line carries "running" too
                s(true, 2, 10, 0),  // duplicate suppressed, unknown key ignored
                s(false, 2, 10, 0), // daemon went away; counters keep last values
                s(true, 2, 10, 0),  // reconnected; replayed state deduplicated
                s(true, 0, 12, 1),  // fresh state
                s(false, 0, 12, 1), // gone again
            ]
        );
        // Both connections carried state lines, so both reconnect delays sit
        // at the floor.
        assert_eq!(sleeps, [RETRY_FLOOR, RETRY_FLOOR]);
    }

    /// A daemon over its watcher cap refuses by closing the adopted
    /// connection with a bare EOF and no line — from this side, a connect
    /// that succeeds and a stream that ends immediately. That must not be
    /// reported as the daemon flapping, and must not retry hot.
    #[test]
    fn a_bare_eof_refusal_publishes_nothing_and_backs_off() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("full.ctl");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            for _ in 0..3 {
                let (conn, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&conn).read_line(&mut line).unwrap();
                assert_eq!(line, "watch\n");
                // Refused: dropped without a byte written.
            }
        });

        let mut sleeps = Vec::new();
        let mut rounds = 0;
        watch_daemon(
            &socket,
            &mut |s| panic!("published {s:?} on a refused watch"),
            &mut |d| sleeps.push(d),
            &mut || {
                rounds += 1;
                rounds <= 3
            },
        );
        server.join().unwrap();
        let secs: Vec<u64> = sleeps.iter().map(Duration::as_secs).collect();
        assert_eq!(secs, [1, 2, 4]);
    }

    /// No daemon: no publishes (the surface already starts as "not
    /// running"), and the retry delay doubles to the ceiling instead of
    /// spinning.
    #[test]
    fn reconnect_backoff_doubles_to_the_ceiling_and_stays_there() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("never-bound.ctl");
        let mut sleeps = Vec::new();
        let mut rounds = 0;
        watch_daemon(
            &socket,
            &mut |s| panic!("published {s:?} without a daemon"),
            &mut |d| sleeps.push(d),
            &mut || {
                rounds += 1;
                rounds <= 8
            },
        );
        let secs: Vec<u64> = sleeps.iter().map(Duration::as_secs).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
    }
}
