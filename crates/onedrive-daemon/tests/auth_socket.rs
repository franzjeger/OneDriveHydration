//! The auth-state socket end to end: a scripted sampler plays the token
//! cache, real clients connect over a real unix socket, and the assertions
//! are on bytes read from it — the same wire the D-Bus service watches.
//!
//! No live credential anywhere (the repository rule): the sampler is a
//! shared struct the test mutates, standing in for the two `TokenCache`
//! accessors whose semantics `credential_semantics.rs` pins separately.

use onedrive_hydration_daemon::auth_state::{self, CredentialHealth, PublisherOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Fast enough that a test waits milliseconds, slow enough that the settle
/// logic (two consecutive identical snapshots) is still two real polls.
const TEST_INTERVAL: Duration = Duration::from_millis(5);

/// How long a test is willing to block on one expected line. Failure mode
/// is a timeout error from the read, never a hung suite.
const READ_DEADLINE: Duration = Duration::from_secs(10);

struct Publisher {
    socket: PathBuf,
    health: Arc<Mutex<CredentialHealth>>,
    adoptions: mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
    served: Option<thread::JoinHandle<std::io::Result<()>>>,
    // Kept alive for the publisher's lifetime; the socket lives here.
    _dir: tempfile::TempDir,
}

impl Publisher {
    /// Serve a publisher over a scripted sampler. `enrollment` names the
    /// file inside the tempdir the adoption logic should watch, when the
    /// test wants one.
    fn start(initial: CredentialHealth, enrollment: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.auth");
        let enrollment = enrollment.map(|name| dir.path().join(name));
        let health = Arc::new(Mutex::new(initial));
        let stop = Arc::new(AtomicBool::new(false));
        let (adopted_tx, adoptions) = mpsc::channel();

        let served = {
            let (socket, health, stop) = (socket.clone(), Arc::clone(&health), Arc::clone(&stop));
            thread::spawn(move || {
                auth_state::serve(
                    &socket,
                    PublisherOptions {
                        sample_interval: TEST_INTERVAL,
                        enrollment,
                    },
                    &mut || *health.lock().unwrap(),
                    &mut || {
                        let _ = adopted_tx.send(());
                    },
                    &mut || !stop.load(Ordering::SeqCst),
                )
            })
        };

        // The socket exists once serve() has bound it; wait for that rather
        // than racing the spawn.
        let deadline = std::time::Instant::now() + READ_DEADLINE;
        while !socket.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the publisher never bound its socket"
            );
            thread::sleep(Duration::from_millis(1));
        }

        Self {
            socket,
            health,
            adoptions,
            stop,
            served: Some(served),
            _dir: dir,
        }
    }

    fn set(&self, health: CredentialHealth) {
        *self.health.lock().unwrap() = health;
    }

    fn enrollment_path(&self) -> PathBuf {
        self._dir.path().join("refresh-token")
    }

    fn connect(&self) -> (UnixStream, BufReader<UnixStream>) {
        let stream = UnixStream::connect(&self.socket).unwrap();
        stream.set_read_timeout(Some(READ_DEADLINE)).unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        (stream, reader)
    }

    fn watch(&self) -> BufReader<UnixStream> {
        let (mut stream, reader) = self.connect();
        stream.write_all(b"watch\n").unwrap();
        reader
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(served) = self.served.take() {
            // The sample loop notices `stop` within one interval; the
            // acceptor thread is deliberately leaked (it blocks in accept,
            // like the daemon's own control thread) and dies with the test
            // process.
            served.join().unwrap().unwrap();
        }
    }
}

fn healthy() -> CredentialHealth {
    CredentialHealth {
        signed_in: true,
        store_error: None,
    }
}

fn rejected() -> CredentialHealth {
    CredentialHealth {
        signed_in: false,
        store_error: None,
    }
}

fn unsaved() -> CredentialHealth {
    CredentialHealth {
        signed_in: true,
        store_error: Some(std::io::ErrorKind::PermissionDenied),
    }
}

fn read_line(reader: &mut BufReader<UnixStream>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    line.trim_end().to_owned()
}

fn write_enrollment(path: &Path, content: &str) {
    // 0600 by construction, matching the legacy migration file contract.
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(content.as_bytes()).unwrap();
}

#[test]
fn watch_answers_immediately_then_once_per_distinct_state() {
    let publisher = Publisher::start(healthy(), None);
    let mut watcher = publisher.watch();

    // One line synchronously at adoption, before anything changes.
    assert_eq!(read_line(&mut watcher), "credential=healthy");

    // A transition produces exactly one line; re-writing the same health is
    // not a transition. Proven by ordering: after a no-op write, the next
    // line read is the *next* state, with nothing in between.
    publisher.set(healthy());
    publisher.set(rejected());
    assert_eq!(read_line(&mut watcher), "credential=rejected");

    publisher.set(unsaved());
    assert_eq!(read_line(&mut watcher), "credential=unsaved");
}

#[test]
fn a_watcher_that_missed_nothing_is_told_the_current_state_not_history() {
    let publisher = Publisher::start(healthy(), None);
    publisher.set(rejected());
    // Give the sample loop a moment to observe the transition, then join.
    thread::sleep(TEST_INTERVAL * 10);
    let mut watcher = publisher.watch();
    assert_eq!(
        read_line(&mut watcher),
        "credential=rejected",
        "a late subscriber gets the state as it is now"
    );
}

#[test]
fn status_answers_in_prose_on_the_same_connection_repeatedly() {
    let publisher = Publisher::start(healthy(), None);
    let (mut stream, mut reader) = publisher.connect();

    stream.write_all(b"status\n").unwrap();
    let answer = read_line(&mut reader);
    assert!(answer.contains("sign-in: healthy"), "{answer}");

    publisher.set(rejected());
    thread::sleep(TEST_INTERVAL * 10);
    stream.write_all(b"status\n").unwrap();
    let answer = read_line(&mut reader);
    assert!(answer.contains("sign-in: REQUIRED"), "{answer}");
    assert!(
        answer.contains("onedrive-hydration-daemon reauth"),
        "{answer}"
    );

    // Unknown verbs answer, not hang — same discipline as the control
    // socket.
    stream.write_all(b"evict x\n").unwrap();
    assert_eq!(read_line(&mut reader), "unknown command: evict x");
}

#[test]
fn a_direct_secret_service_enrollment_requests_one_restart() {
    let publisher = Publisher::start(healthy(), None);
    let main_socket = publisher.socket.with_extension("sock");

    auth_state::notify_enrollment(&main_socket).unwrap();
    publisher
        .adoptions
        .recv_timeout(READ_DEADLINE)
        .expect("the daemon did not restart for the direct enrollment");
    assert!(
        publisher.adoptions.try_recv().is_err(),
        "one notification must request one restart"
    );
}

#[test]
fn watchers_over_the_cap_are_refused_with_a_bare_eof() {
    let publisher = Publisher::start(healthy(), None);
    // The cap is eight; hold eight live watchers.
    let mut held = Vec::new();
    for _ in 0..8 {
        let mut watcher = publisher.watch();
        assert_eq!(read_line(&mut watcher), "credential=healthy");
        held.push(watcher);
    }
    // The ninth is closed without a line — the same refusal the control
    // socket uses, which the reading side already maps to retry-with-
    // backoff rather than an error.
    let mut refused = publisher.watch();
    let mut line = String::new();
    let read = refused.read_line(&mut line).unwrap();
    assert_eq!(read, 0, "refusal is EOF, got {line:?}");

    // Slots are reclaimed when watchers hang up, so a refusal is about the
    // population, not the connection's place in history.
    held.clear();
    thread::sleep(TEST_INTERVAL * 10);
    let mut watcher = publisher.watch();
    assert_eq!(read_line(&mut watcher), "credential=healthy");
}

#[test]
fn a_settled_enrollment_is_adopted_only_while_rejected_and_only_once() {
    let publisher = Publisher::start(healthy(), Some("refresh-token"));
    let path = publisher.enrollment_path();

    // Healthy: the file waits for the next natural restart, however long it
    // sits there.
    write_enrollment(&path, "fresh-signin-bytes");
    assert!(
        publisher
            .adoptions
            .recv_timeout(TEST_INTERVAL * 20)
            .is_err(),
        "a working sign-in must not be switched out from under a running sync"
    );

    // Rejected: the settled file is adopted (the binary's hook restarts the
    // daemon; the test's hook records the call).
    publisher.set(rejected());
    publisher
        .adoptions
        .recv_timeout(READ_DEADLINE)
        .expect("a settled enrollment is adopted while rejected");

    // And once only for the same bytes: the binary never returns from its
    // hook, but the logic must not depend on that.
    assert!(
        publisher
            .adoptions
            .recv_timeout(TEST_INTERVAL * 20)
            .is_err(),
        "the same settled file must not be adopted twice"
    );
}

#[test]
fn an_unadoptable_enrollment_file_is_left_alone() {
    let publisher = Publisher::start(rejected(), Some("refresh-token"));
    let path = publisher.enrollment_path();

    // Group-readable: the startup migration would refuse it, so restarting
    // onto it would take the daemon down. The publisher must not.
    write_enrollment(&path, "exposed-bytes");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        publisher
            .adoptions
            .recv_timeout(TEST_INTERVAL * 20)
            .is_err(),
        "a file the migration would refuse must not trigger a restart"
    );

    // Tightening the permissions makes it adoptable without rewriting it.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    publisher
        .adoptions
        .recv_timeout(READ_DEADLINE)
        .expect("the repaired file is adopted");
}
