use hydration_graph::auth::{AuthConfig, TokenCache};
use hydration_graph::{
    DriveId, FileCredentialStore, GraphTokens, Method, MonotonicClock, Reply, Request,
    SharedTokenCache, Transport,
};
use serde::Deserialize;
use std::io;
use std::path::Path;
use std::sync::Arc;

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

pub fn token_cache(config: AuthConfig, credential: &Path) -> SharedTokenCache {
    Arc::new(TokenCache::new(
        config,
        Arc::new(GraphTokens::new()),
        MonotonicClock,
        FileCredentialStore::new(credential),
    ))
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
    use std::time::Duration;

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
}
