pub mod dbus;

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
            .map_err(|_| secret_service_error("could not connect to Linux Secret Service"))
    }
}

impl SecretBackend for KeyringBackend {
    fn load(&self) -> io::Result<Option<String>> {
        match self.0.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(secret_service_error(
                "could not read the OneDrive credential from Linux Secret Service",
            )),
        }
    }

    fn save(&self, value: &str) -> io::Result<()> {
        self.0.set_password(value).map_err(|_| {
            secret_service_error(
                "could not persist the rotated OneDrive credential in Linux Secret Service",
            )
        })
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

fn secret_service_error(message: &'static str) -> io::Error {
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

fn migrate_legacy_credential(store: &dyn CredentialStore, legacy_path: &Path) -> io::Result<()> {
    let legacy = FileCredentialStore::new(legacy_path);
    let secure_exists = store.load()?.is_some();
    if !secure_exists {
        if legacy_path.exists() {
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
        }
        if let Some(refresh) = legacy.load()? {
            store.save(&refresh)?;
        } else {
            return Ok(());
        }
    } else if !legacy_path.exists() {
        return Ok(());
    }

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
    fn secure_credential_wins_over_stale_legacy_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh-token");
        FileCredentialStore::new(&path)
            .save(&RefreshToken::new("stale-legacy"))
            .unwrap();
        let store = SecretServiceCredentialStore(MemorySecret::default());
        store.save(&RefreshToken::new("secure-current")).unwrap();

        migrate_legacy_credential(&store, &path).unwrap();

        assert!(!path.exists());
        assert_eq!(
            store.load().unwrap().unwrap().expose_for_storage(),
            "secure-current"
        );
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
