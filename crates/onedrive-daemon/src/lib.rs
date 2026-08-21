pub mod auth_state;
pub mod dbus;
pub mod pkce;
pub mod tray;

use hydration_graph::auth::{AuthConfig, CredentialStore, RefreshToken, TokenCache};
use hydration_graph::{
    DriveId, FileCredentialStore, GraphTokens, Method, MonotonicClock, Reply, Request,
    SharedCredentialStore, SharedTokenCache, Transport,
};
use serde::Deserialize;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const ME_DRIVE: &str = "https://graph.microsoft.com/v1.0/me/drive?$select=id,driveType";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveProfile {
    pub id: DriveId,
    pub drive_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveReply {
    id: String,
    drive_type: String,
}

pub fn auth_config(client_id: String) -> AuthConfig {
    AuthConfig::public_client(client_id).with_scopes(["Files.ReadWrite.All", "User.Read"])
}

const CREDENTIAL_SERVICE: &str = "io.github.franzjeger.OneDriveHydration";

pub fn runtime_socket(file_name: &str) -> io::Result<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "XDG_RUNTIME_DIR is not set; pass --socket explicitly",
        )
    })?;
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() || !dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "XDG_RUNTIME_DIR is not an absolute existing directory",
        ));
    }
    Ok(dir.join(file_name))
}

pub fn control_request(socket: &Path, command: &str) -> io::Result<String> {
    if command.is_empty() || command.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "control command is empty or contains a forbidden character",
        ));
    }
    let mut stream = UnixStream::connect(socket)?;
    let timeout = Some(Duration::from_secs(10));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    stream.write_all(command.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    if reply.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the daemon closed the control connection without a reply",
        ));
    }
    Ok(reply.trim_end_matches(['\n', '\r']).to_owned())
}

trait SecretBackend: Send + Sync {
    fn load(&self) -> io::Result<Option<String>>;
    fn save(&self, value: &str) -> io::Result<()>;
}

struct KeyringBackend(keyring::Entry);

impl KeyringBackend {
    fn new(user: &str) -> io::Result<Self> {
        keyring::Entry::new(CREDENTIAL_SERVICE, user)
            .map(Self)
            .map_err(|e| {
                secret_service_error(format!("could not connect to Linux Secret Service: {e}"))
            })
    }
}

// The keyring error is kept in every message below. It never carries secret
// material — read failures have no secret to quote, and the write errors
// describe the store, not the value — and discarding it once made a login
// race ("the object does not exist yet") indistinguishable in the journal
// from a corrupted store. Print what actually happened.
impl SecretBackend for KeyringBackend {
    fn load(&self) -> io::Result<Option<String>> {
        match self.0.get_password() {
            Ok(value) => Ok(Some(value)),
            // Authoritative: the store answered, and the answer is "no such
            // credential". Mapped to None — signed out — never to an error,
            // and never retried; only a store that *cannot answer* is worth
            // waiting for (see [`wait_for_secret_service`]).
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(secret_service_error(format!(
                "could not read the OneDrive credential from Linux Secret Service: {e}"
            ))),
        }
    }

    fn save(&self, value: &str) -> io::Result<()> {
        self.0.set_password(value).map_err(|e| {
            secret_service_error(format!(
                "could not persist the rotated OneDrive credential in Linux Secret Service: {e}"
            ))
        })
    }
}

/// The well-known name of the credential store, fixed by the freedesktop
/// Secret Service specification.
pub const SECRET_SERVICE_BUS_NAME: &str = "org.freedesktop.secrets";

/// How long [`wait_for_secret_service`] is willing to wait. The race it
/// covers was measured at ~5s (see [`wait_for_secret_service`]); a minute
/// absorbs a slow desktop several times over while still turning a genuinely
/// absent store into a precise error within the first restart cycle or two.
pub const SECRET_SERVICE_WAIT: Duration = Duration::from_secs(60);

/// How often the wait looks again. Half a second keeps the measured ~5s race
/// from costing more than it must, and sixty seconds of half-second glances
/// at the bus driver cost nothing anyone can observe.
const STORE_POLL: Duration = Duration::from_millis(500);

/// One glance at the session bus: could the credential store answer a call
/// right now?
enum StoreSight {
    /// It could — the name is owned, or the bus can activate it (in which
    /// case the first real call starts it and is queued until it is up).
    Present(&'static str),
    /// It could not, and here is exactly what was seen instead.
    Absent(String),
}

fn secret_service_sight(connection: &mut Option<zbus::blocking::Connection>) -> StoreSight {
    let conn = match connection {
        Some(conn) => conn,
        None => match zbus::blocking::Connection::session() {
            Ok(conn) => connection.insert(conn),
            Err(e) => {
                return StoreSight::Absent(format!("could not connect to the session bus: {e}"))
            }
        },
    };
    let Ok(name) = zbus::names::BusName::try_from(SECRET_SERVICE_BUS_NAME) else {
        // A constant that stopped parsing is a build defect; there is no
        // point retrying it, but failing closed with the reason beats a
        // panic in a daemon.
        return StoreSight::Absent(format!("{SECRET_SERVICE_BUS_NAME} is not a valid bus name"));
    };
    let fdo = match zbus::blocking::fdo::DBusProxy::new(conn) {
        Ok(fdo) => fdo,
        Err(e) => {
            *connection = None;
            return StoreSight::Absent(format!("could not reach the bus driver: {e}"));
        }
    };
    match fdo.name_has_owner(name) {
        Ok(true) => return StoreSight::Present("owned"),
        Ok(false) => {}
        Err(e) => {
            // The connection may be the casualty; rebuild it next glance.
            *connection = None;
            return StoreSight::Absent(format!(
                "the bus could not say whether {SECRET_SERVICE_BUS_NAME} is owned: {e}"
            ));
        }
    }
    match fdo.list_activatable_names() {
        Ok(names) if names.iter().any(|n| n.as_str() == SECRET_SERVICE_BUS_NAME) => {
            StoreSight::Present("activatable")
        }
        Ok(_) => StoreSight::Absent(format!(
            "{SECRET_SERVICE_BUS_NAME} has no owner and is not activatable on the session bus"
        )),
        Err(e) => {
            *connection = None;
            StoreSight::Absent(format!("the bus could not list activatable names: {e}"))
        }
    }
}

/// Wait, bounded, for the credential store to be able to answer.
///
/// Why this lives in the daemon and not in its unit: measured at login on the
/// verified deployment (2026-08-12), the user manager reached `default.target`
/// and started the daemon at t=20.7s, while `org.freedesktop.secrets` appeared
/// at t=25.8s — owned by `ksecretd`, which PAM starts inside the login
/// session's scope (`session-N.scope`, `UserUnit=n/a`) with the login password
/// on inherited file descriptors. There is no user unit an `After=` could
/// name, the bus name is not activatable so the bus cannot summon it, and an
/// ordering against a unit outside the daemon's own start transaction orders
/// nothing anyway. So the process that needs the store is the one that waits
/// for it — and says so, once, rather than exiting with an error a reader
/// cannot tell from a missing credential.
///
/// A store that answers "no such credential" is *not* waited for: `NoEntry` is
/// an authoritative answer — the store is up and it has looked — and papering
/// over it here would turn "you need to sign in" into a silent minute of
/// nothing. The lookup itself is `SecretBackend::load`, which is private, so
/// this names it in prose rather than linking to it; a doc link to a private
/// item fails `cargo doc -D warnings`, which CI runs.
pub fn wait_for_secret_service(bound: Duration) -> io::Result<()> {
    let mut connection = None;
    wait_for_store(
        bound,
        &mut || secret_service_sight(&mut connection),
        &mut std::thread::sleep,
        &mut |line| eprintln!("{line}"),
    )
}

/// The loop and the judgment, separated from the bus so a test can hand it
/// sights this machine cannot produce. Time is counted in sleeps actually
/// requested, not read from a clock, so the tests neither wait nor race.
fn wait_for_store(
    bound: Duration,
    look: &mut dyn FnMut() -> StoreSight,
    sleep: &mut dyn FnMut(Duration),
    log: &mut dyn FnMut(&str),
) -> io::Result<()> {
    let mut waited = Duration::ZERO;
    let mut announced = false;
    loop {
        match look() {
            StoreSight::Present(how) => {
                if announced {
                    log(&format!(
                        "onedrive-hydration-daemon: Linux Secret Service became available \
                         after {:.1}s ({how})",
                        waited.as_secs_f64()
                    ));
                }
                return Ok(());
            }
            StoreSight::Absent(detail) if waited >= bound => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Linux Secret Service did not come up within {}s: {detail}. The \
                         credential store itself is unavailable — that is not a missing \
                         credential, and enrolling again will not help. A Secret Service \
                         provider (ksecretd, gnome-keyring) normally starts with the \
                         desktop session; without one this daemon fails closed on purpose",
                        bound.as_secs()
                    ),
                ));
            }
            StoreSight::Absent(detail) => {
                // Once. A line per poll would be five thousand copies of the
                // same fact on a store that never comes up.
                if !announced {
                    log(&format!(
                        "onedrive-hydration-daemon: the credential store is not up yet \
                         ({detail}); waiting up to {}s for the session to bring it up",
                        bound.as_secs()
                    ));
                    announced = true;
                }
            }
        }
        sleep(STORE_POLL);
        waited += STORE_POLL;
    }
}

struct SecretServiceCredentialStore<B>(B);

impl<B: SecretBackend> CredentialStore for SecretServiceCredentialStore<B> {
    fn load(&self) -> io::Result<Option<RefreshToken>> {
        self.0.load().map(|value| value.map(RefreshToken::new))
    }

    fn save(&self, refresh: &RefreshToken) -> io::Result<()> {
        self.0.save(refresh.expose_for_storage())
    }
}

fn secret_service_error(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

pub fn token_cache(config: AuthConfig, state_dir: &Path) -> io::Result<SharedTokenCache> {
    // Isolate credentials created for different Azure app registrations. The
    // client id is public configuration, not a secret.
    let user = format!("refresh-token:{}", config.client_id());
    let store: SharedCredentialStore =
        Arc::new(SecretServiceCredentialStore(KeyringBackend::new(&user)?));
    migrate_legacy_credential(store.as_ref(), &state_dir.join("refresh-token"))?;
    Ok(Arc::new(TokenCache::new(
        config,
        Arc::new(GraphTokens::new()),
        MonotonicClock,
        store,
    )))
}

/// Store a newly enrolled refresh token directly in Linux Secret Service.
///
/// Browser enrollment uses this instead of the legacy state-directory handoff,
/// so the credential is never written to a plaintext filesystem path. The
/// account key is exactly the one [`token_cache`] uses on the next start.
pub fn store_enrolled_credential(config: &AuthConfig, refresh: &RefreshToken) -> io::Result<()> {
    let user = format!("refresh-token:{}", config.client_id());
    SecretServiceCredentialStore(KeyringBackend::new(&user)?).save(refresh)
}

/// Adopt a credential file left in the state directory, then remove it.
///
/// Old builds produced that file through the file-backed alpha or the legacy
/// external browser helper. Current browser enrollment writes Secret Service
/// directly. The daemon still consumes a leftover file on every start, so an
/// upgrade cannot strand the credential that made the old deployment work.
///
/// The file wins over a stored credential on purpose, and this reverses an
/// earlier rule ("the secure credential wins over a stale legacy file").
/// That rule had exactly one reachable consequence: when the service had
/// rejected the stored credential — the one situation in which anyone
/// re-enrolls — the fresh sign-in was deleted unread and the dead credential
/// kept, so following the product's own re-authentication instructions
/// landed the user back where they started, minus the sign-in they had just
/// completed. A genuinely stale file (a restored state-dir backup) costs a
/// few `invalid_grant`s and a visible "sign-in required", which is the same
/// recovery path and loses nothing.
fn migrate_legacy_credential(store: &dyn CredentialStore, legacy_path: &Path) -> io::Result<()> {
    if !legacy_path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(legacy_path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "legacy credential is not a private regular file",
        ));
    }
    let Some(refresh) = FileCredentialStore::new(legacy_path).load()? else {
        return Ok(());
    };
    store.save(&refresh)?;

    // The secure write above completed before plaintext removal. Failing to
    // remove or durably record the removal is an error, not a silent warning.
    fs::remove_file(legacy_path)?;
    let parent = legacy_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "legacy credential path has no parent",
        )
    })?;
    fs::File::open(parent)?.sync_all()
}

/// What the auth-state publisher needs to know about a possible enrollment
/// file without reading it: is it adoptable, and has it stopped changing?
///
/// Metadata only, deliberately. The publisher's job is to notice that a
/// fresh enrollment exists and restart the daemon so the startup path —
/// [`migrate_legacy_credential`], the only code that reads credential bytes
/// — adopts it; a second reader of the credential would be a second thing
/// to audit. The size/mtime pair is how the publisher tells a settled file
/// from one the enrollment tool is still writing.
pub(crate) fn enrollment_snapshot(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = fs::symlink_metadata(path).ok()?;
    let private_regular = metadata.file_type().is_file()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o077 == 0
        && metadata.len() > 0;
    if !private_regular {
        return None;
    }
    Some((metadata.len(), metadata.modified().ok()?))
}

pub fn discover_drive(transport: &mut impl Transport) -> io::Result<DriveProfile> {
    let reply = transport.send(&Request::new(Method::Get, ME_DRIVE))?;
    parse_drive_reply(reply)
}

fn parse_drive_reply(reply: Reply) -> io::Result<DriveProfile> {
    if !(200..300).contains(&reply.status) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("Graph drive discovery returned HTTP {}", reply.status),
        ));
    }
    let raw: DriveReply = serde_json::from_slice(&reply.body).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Graph drive discovery returned a malformed reply",
        )
    })?;
    if !matches!(
        raw.drive_type.as_str(),
        "personal" | "business" | "documentLibrary"
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Graph drive discovery returned an unsupported drive type",
        ));
    }
    let id = DriveId::parse(&raw.id).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Graph drive discovery returned an invalid drive id",
        )
    })?;
    Ok(DriveProfile {
        id,
        drive_type: raw.drive_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;

    #[derive(Default)]
    struct MemorySecret(Mutex<Option<String>>);

    impl SecretBackend for MemorySecret {
        fn load(&self) -> io::Result<Option<String>> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn save(&self, value: &str) -> io::Result<()> {
            *self.0.lock().unwrap() = Some(value.to_owned());
            Ok(())
        }
    }

    struct Wire {
        replies: VecDeque<Reply>,
        requests: Vec<Request>,
    }

    impl Transport for Wire {
        fn send(&mut self, request: &Request) -> io::Result<Reply> {
            self.requests.push(request.clone());
            self.replies
                .pop_front()
                .ok_or_else(|| io::Error::other("no scripted reply"))
        }
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            retry_after: Some(Duration::from_secs(1)),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn discovers_the_signed_in_users_drive() {
        let mut wire = Wire {
            replies: VecDeque::from([reply(200, r#"{"id":"b!drive","driveType":"business"}"#)]),
            requests: Vec::new(),
        };
        let profile = discover_drive(&mut wire).unwrap();
        assert_eq!(profile.id.as_str(), "b!drive");
        assert_eq!(profile.drive_type, "business");
        assert_eq!(wire.requests.len(), 1);
        assert_eq!(wire.requests[0].method, Method::Get);
        assert_eq!(wire.requests[0].url, ME_DRIVE);
        assert!(wire.requests[0].authorize);
    }

    #[test]
    fn malformed_reply_fails_closed() {
        let err = parse_drive_reply(reply(200, r#"{"driveType":"business"}"#)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn unsupported_drive_type_fails_closed() {
        let err = parse_drive_reply(reply(200, r#"{"id":"drive","driveType":"futureType"}"#))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn error_body_is_not_copied_into_the_error() {
        let secret = "do-not-log-this-body";
        let err = parse_drive_reply(reply(401, secret)).unwrap_err();
        assert!(!err.to_string().contains(secret));
    }

    #[test]
    fn secret_service_adapter_round_trips_rotated_credentials() {
        let store = SecretServiceCredentialStore(MemorySecret::default());
        store.save(&RefreshToken::new("rotated-refresh")).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.expose_for_storage(), "rotated-refresh");
    }

    #[test]
    fn missing_secret_is_signed_out_not_an_empty_credential() {
        let store = SecretServiceCredentialStore(MemorySecret::default());
        assert!(store.load().unwrap().is_none());
    }

    // --- the bounded wait for the credential store ------------------------
    //
    // The loop is exercised with scripted sights and counted sleeps: no bus,
    // no clock, no live credential — per the repository rule — and the
    // assertions are on the load-bearing words of the messages, because the
    // messages are the fix (a wait that failed with the old text would still
    // read as "sign in again").

    #[test]
    fn store_wait_is_silent_when_the_store_is_already_up() {
        let mut looks = 0;
        let mut slept = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        wait_for_store(
            Duration::from_secs(60),
            &mut || {
                looks += 1;
                StoreSight::Present("owned")
            },
            &mut |d| slept.push(d),
            &mut |l| lines.push(l.to_owned()),
        )
        .unwrap();
        assert_eq!(looks, 1);
        assert!(slept.is_empty(), "a present store must cost no sleep");
        assert!(
            lines.is_empty(),
            "the healthy path must not chatter: {lines:?}"
        );
    }

    #[test]
    fn store_wait_outlasts_the_login_race_and_narrates_it_once() {
        let mut sights = VecDeque::from([
            StoreSight::Absent("org.freedesktop.secrets has no owner".into()),
            StoreSight::Absent("org.freedesktop.secrets has no owner".into()),
            StoreSight::Present("owned"),
        ]);
        let mut slept = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        wait_for_store(
            Duration::from_secs(60),
            &mut || sights.pop_front().expect("the script covers every look"),
            &mut |d| slept.push(d),
            &mut |l| lines.push(l.to_owned()),
        )
        .unwrap();
        assert_eq!(slept.len(), 2, "one sleep per absent sight");
        assert_eq!(
            lines.len(),
            2,
            "one announcement, one resolution: {lines:?}"
        );
        assert!(lines[0].contains("not up yet"), "{}", lines[0]);
        assert!(lines[0].contains("waiting up to 60s"), "{}", lines[0]);
        assert!(
            lines[0].contains("has no owner"),
            "the announcement carries what was actually seen: {}",
            lines[0]
        );
        assert!(
            lines[1].contains("became available after 1.0s"),
            "{}",
            lines[1]
        );
    }

    #[test]
    fn store_wait_deadline_is_a_store_outage_not_a_missing_credential() {
        let mut slept = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        let err = wait_for_store(
            Duration::from_secs(3),
            &mut || {
                StoreSight::Absent(
                    "org.freedesktop.secrets has no owner and is not activatable on the \
                     session bus"
                        .into(),
                )
            },
            &mut |d| slept.push(d),
            &mut |l| lines.push(l.to_owned()),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        let msg = err.to_string();
        assert!(msg.contains("did not come up within 3s"), "{msg}");
        assert!(msg.contains("not activatable"), "{msg}");
        // The words that keep a reader from re-enrolling over an outage.
        assert!(msg.contains("not a missing credential"), "{msg}");
        assert!(msg.contains("enrolling again will not help"), "{msg}");
        // Bounded in sleeps, not wall clock: 3s at 500ms per look.
        assert_eq!(slept.len(), 6);
        assert_eq!(
            lines.len(),
            1,
            "the wait announces once, not per poll: {lines:?}"
        );
    }

    #[test]
    fn keyring_failures_keep_the_underlying_error_visible() {
        // The mapping itself: whatever the platform said must survive into
        // the io::Error a journal shows, or a login race and a corrupted
        // store read identically (which is the defect this repository
        // measured on 2026-08-12).
        let err = secret_service_error(format!(
            "could not read the OneDrive credential from Linux Secret Service: {}",
            keyring::Error::Invalid("session".into(), "collection is locked".into())
        ));
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("collection is locked"), "{err}");
    }

    #[test]
    fn legacy_file_is_removed_only_after_secure_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-token");
        FileCredentialStore::new(&path)
            .save(&RefreshToken::new("legacy-refresh"))
            .unwrap();
        let store = SecretServiceCredentialStore(MemorySecret::default());

        migrate_legacy_credential(&store, &path).unwrap();

        assert!(!path.exists());
        assert_eq!(
            store.load().unwrap().unwrap().expose_for_storage(),
            "legacy-refresh"
        );
    }

    #[test]
    fn a_fresh_enrollment_replaces_the_stored_credential() {
        // The situation this models is the only one that produces both at
        // once on an old build: the service rejected the stored credential,
        // the user ran the legacy browser helper, and the daemon restarted. The
        // file is the enrollment; deleting it and keeping the rejected
        // credential — the previous rule — made the product's own
        // re-authentication instructions a trap.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-token");
        FileCredentialStore::new(&path)
            .save(&RefreshToken::new("fresh-enrollment"))
            .unwrap();
        let store = SecretServiceCredentialStore(MemorySecret::default());
        store
            .save(&RefreshToken::new("rejected-by-the-service"))
            .unwrap();

        migrate_legacy_credential(&store, &path).unwrap();

        assert!(!path.exists());
        assert_eq!(
            store.load().unwrap().unwrap().expose_for_storage(),
            "fresh-enrollment"
        );
    }

    #[test]
    fn a_stored_credential_is_kept_when_there_is_no_enrollment_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretServiceCredentialStore(MemorySecret::default());
        store.save(&RefreshToken::new("secure-current")).unwrap();

        migrate_legacy_credential(&store, &dir.path().join("refresh-token")).unwrap();

        assert_eq!(
            store.load().unwrap().unwrap().expose_for_storage(),
            "secure-current"
        );
    }

    #[test]
    fn enrollment_snapshot_accepts_only_a_settled_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-token");

        // Absent: nothing to adopt.
        assert!(enrollment_snapshot(&path).is_none());

        // Empty: the enrollment tool never writes an empty file, so this is
        // a write in progress or a crashed one — not adoptable.
        fs::write(&path, "").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(enrollment_snapshot(&path).is_none());

        // Group/other-readable: the migration would refuse it at startup,
        // so restarting the daemon over it would take the daemon down.
        fs::write(&path, "token-bytes").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(enrollment_snapshot(&path).is_none());

        // Private and non-empty: adoptable, and the snapshot is what the
        // publisher compares across polls to see the file settle.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let first = enrollment_snapshot(&path).expect("a private file has a snapshot");
        assert_eq!(first.0, "token-bytes".len() as u64);
        assert_eq!(enrollment_snapshot(&path), Some(first));
    }

    #[test]
    fn permissive_legacy_file_is_refused_without_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-token");
        fs::write(&path, "exposed-refresh").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let store = SecretServiceCredentialStore(MemorySecret::default());

        let err = migrate_legacy_credential(&store, &path).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(path.exists());
        assert!(store.load().unwrap().is_none());
        assert!(!err.to_string().contains("exposed-refresh"));
    }

    #[test]
    fn control_client_sends_one_exact_command_and_reads_multiline_reply() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut command = String::new();
            BufReader::new(conn.try_clone().unwrap())
                .read_line(&mut command)
                .unwrap();
            assert_eq!(command, "evict Documents/report final.pdf\n");
            conn.write_all(b"reclaimed 4096 bytes\nsecond line\n")
                .unwrap();
        });

        let reply = control_request(&socket, "evict Documents/report final.pdf").unwrap();

        assert_eq!(reply, "reclaimed 4096 bytes\nsecond line");
        server.join().unwrap();
    }

    #[test]
    fn control_client_refuses_command_injection_before_connecting() {
        for command in ["", "status\nevict secret", "status\r", "status\0"] {
            let err = control_request(Path::new("/does/not/exist"), command).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
    }
}
