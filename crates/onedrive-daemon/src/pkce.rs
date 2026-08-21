//! Browser authorization-code enrollment with PKCE and a loopback redirect.
//!
//! The listener is bound before its port is advertised, uses the IPv4 literal
//! `127.0.0.1` on both sides, validates `state` before spending the code, and
//! never writes the resulting refresh token to disk. Microsoft permits an
//! ephemeral loopback port for native public clients and requires the same
//! redirect URI and PKCE verifier at redemption.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hydration_graph::auth::RefreshToken;
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use ring::digest::{digest, SHA256};
use serde::Deserialize;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const AUTHORITY: &str = "login.microsoftonline.com";
const TENANT: &str = "common";
const SCOPE: &str = "offline_access Files.ReadWrite.All User.Read";
const CALLBACK_LIMIT: usize = 16 * 1024;
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
struct TokenResponse {
    refresh_token: Option<String>,
    scope: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// A bound one-shot enrollment. Keeping the listener in this value closes the
/// usual "find a free port, close it, bind it later" race.
pub struct BrowserEnrollment {
    listener: TcpListener,
    redirect_uri: String,
    authorize_url: String,
    verifier: String,
    state: String,
    client_id: String,
}

impl BrowserEnrollment {
    pub fn begin(client_id: &str) -> io::Result<Self> {
        if client_id.is_empty() || client_id.bytes().any(|b| b.is_ascii_whitespace()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the client id is empty or contains whitespace",
            ));
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}");
        let verifier = random_b64(32)?;
        let state = random_b64(24)?;
        let challenge = URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()).as_ref());
        let authorize_url = format!(
            "https://{AUTHORITY}/{TENANT}/oauth2/v2.0/authorize?{}",
            form(&[
                ("client_id", client_id),
                ("response_type", "code"),
                ("redirect_uri", &redirect_uri),
                ("response_mode", "query"),
                ("scope", SCOPE),
                ("state", &state),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
                ("prompt", "select_account"),
            ])
        );
        Ok(Self {
            listener,
            redirect_uri,
            authorize_url,
            verifier,
            state,
            client_id: client_id.to_owned(),
        })
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn authorize_url(&self) -> &str {
        &self.authorize_url
    }

    /// Wait for the browser callback, then redeem its code through the supplied
    /// exchange seam. Tests inject the seam; production uses [`exchange_code`].
    fn complete_with(
        self,
        timeout: Duration,
        exchange: impl FnOnce(&str, &[(&str, &str)]) -> io::Result<TokenResponse>,
        store: impl FnOnce(&RefreshToken) -> io::Result<()>,
    ) -> io::Result<Option<String>> {
        let deadline = Instant::now() + timeout;
        let (mut stream, _) = loop {
            match self.listener.accept() {
                Ok(pair) => break pair,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "no browser redirect arrived within 300 seconds; a sandboxed browser may be unable to reach the host loopback listener—copy the printed URL into a host browser and try again",
                    ));
                }
                Err(e) => return Err(e),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let target = read_request_target(&mut stream)?;
        let query = target.strip_prefix("/?").ok_or_else(|| {
            callback_error("the redirect did not target the registered root path")
        })?;
        let fields = parse_query(query)?;

        if one(&fields, "state")? != Some(self.state.as_str()) {
            respond(
                &mut stream,
                400,
                "This response did not belong to the current sign-in.",
            )?;
            return Err(callback_error(
                "the browser redirect state did not match this enrollment",
            ));
        }
        if let Some(error) = one(&fields, "error")? {
            respond(
                &mut stream,
                400,
                "Sign-in was not completed. Return to the application.",
            )?;
            let description = one(&fields, "error_description")?.unwrap_or("no description");
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "Microsoft sign-in returned {}: {}",
                    safe_diagnostic(error),
                    safe_diagnostic(description)
                ),
            ));
        }
        let code = match one(&fields, "code")? {
            Some(code) => code,
            None => {
                respond(
                    &mut stream,
                    400,
                    "The sign-in response contained no authorization code.",
                )?;
                return Err(callback_error(
                    "the browser redirect contained neither an authorization code nor an error",
                ));
            }
        };
        let response = match exchange(
            &format!("https://{AUTHORITY}/{TENANT}/oauth2/v2.0/token"),
            &[
                ("client_id", &self.client_id),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect_uri),
                ("code_verifier", &self.verifier),
                ("scope", SCOPE),
            ],
        ) {
            Ok(response) => response,
            Err(error) => {
                let _ = respond(
                    &mut stream,
                    500,
                    "The sign-in could not be completed. Return to OneDrive Hydration for details.",
                );
                return Err(error);
            }
        };
        if let Some(error) = response.error {
            respond(
                &mut stream,
                400,
                "Microsoft refused the token exchange. Return to OneDrive Hydration for details.",
            )?;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "Microsoft token exchange returned {}: {}",
                    safe_diagnostic(&error),
                    safe_diagnostic(
                        response
                            .error_description
                            .as_deref()
                            .unwrap_or("no description")
                    )
                ),
            ));
        }
        let refresh = match response.refresh_token {
            Some(refresh) => RefreshToken::new(refresh),
            None => {
                respond(
                    &mut stream,
                    400,
                    "Microsoft returned no reusable sign-in. Return to OneDrive Hydration for details.",
                )?;
                return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Microsoft token exchange returned no refresh token; verify offline_access consent and the public-client redirect registration",
                ));
            }
        };
        if let Err(error) = store(&refresh) {
            let _ = respond(
                &mut stream,
                500,
                "Sign-in succeeded, but Linux Secret Service could not store it. Unlock the keyring and try again.",
            );
            return Err(error);
        }
        respond(
            &mut stream,
            200,
            "Sign-in completed and was stored securely. You can close this tab.",
        )?;
        Ok(response.scope)
    }

    /// Complete enrollment and install the refresh token before acknowledging
    /// success in the browser. The token exists only in this process and the
    /// supplied secure-store implementation; it is never returned to a UI or
    /// written to an intermediate file.
    pub fn complete(
        self,
        store: impl FnOnce(&RefreshToken) -> io::Result<()>,
    ) -> io::Result<Option<String>> {
        self.complete_with(CALLBACK_TIMEOUT, exchange_code, store)
    }
}

pub fn open_browser(url: &str) -> io::Result<()> {
    match Command::new("xdg-open").arg(url).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(io::Error::other(format!(
            "xdg-open exited with {status}; open the printed URL manually"
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "xdg-open is not installed; open the printed URL manually",
        )),
        Err(e) => Err(e),
    }
}

fn exchange_code(url: &str, fields: &[(&str, &str)]) -> io::Result<TokenResponse> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build();
    let agent: ureq::Agent = config.into();
    let mut response = agent
        .post(url)
        .send_form(fields.iter().copied())
        .map_err(|e| io::Error::other(format!("the token endpoint could not be reached: {e}")))?;
    let status = response.status().as_u16();
    let raw = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_vec()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "the token reply could not be read",
            )
        })?;
    serde_json::from_slice(&raw).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the token endpoint returned malformed JSON (HTTP {status})"),
        )
    })
}

fn random_b64(bytes: usize) -> io::Result<String> {
    let mut raw = vec![0_u8; bytes];
    File::open("/dev/urandom")?.read_exact(&mut raw)?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

fn encode(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn parse_query(query: &str) -> io::Result<Vec<(String, String)>> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            let decode = |raw: &str| {
                percent_decode_str(&raw.replace('+', " "))
                    .decode_utf8()
                    .map(|value| value.into_owned())
                    .map_err(|_| callback_error("the browser redirect query was not UTF-8"))
            };
            Ok((decode(key)?, decode(value)?))
        })
        .collect()
}

fn one<'a>(fields: &'a [(String, String)], key: &str) -> io::Result<Option<&'a str>> {
    let mut found = fields
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, v)| v.as_str());
    let first = found.next();
    if found.next().is_some() {
        return Err(callback_error(
            "the browser redirect repeated a security-sensitive field",
        ));
    }
    Ok(first)
}

fn read_request_target(stream: &mut TcpStream) -> io::Result<String> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(callback_error(
                "the browser closed the redirect before sending headers",
            ));
        }
        request.extend_from_slice(&chunk[..count]);
        if request.len() > CALLBACK_LIMIT {
            return Err(callback_error(
                "the browser redirect headers were too large",
            ));
        }
    }
    let first = request
        .split(|b| *b == b'\n')
        .next()
        .ok_or_else(|| callback_error("the browser redirect had no request line"))?;
    let first = std::str::from_utf8(first)
        .map_err(|_| callback_error("the browser redirect request line was not ASCII"))?
        .trim_end_matches('\r');
    let mut words = first.split_whitespace();
    if words.next() != Some("GET") {
        return Err(callback_error("the browser redirect was not an HTTP GET"));
    }
    let target = words
        .next()
        .ok_or_else(|| callback_error("the browser redirect had no request target"))?;
    if words.next() != Some("HTTP/1.1") || words.next().is_some() {
        return Err(callback_error(
            "the browser redirect had a malformed request line",
        ));
    }
    Ok(target.to_owned())
}

fn respond(stream: &mut TcpStream, status: u16, message: &str) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Internal Server Error",
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>OneDrive Hydration</title><h2>{}</h2>",
        message
    );
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    )
}

fn callback_error(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Keep service-controlled diagnostics useful without permitting terminal or
/// journal control injection, and without copying an unbounded response.
fn safe_diagnostic(value: &str) -> String {
    value
        .chars()
        .take(512)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn pkce_uses_the_rfc_7636_s256_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()).as_ref()),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn listener_is_bound_before_the_url_is_returned_and_uses_the_literal_address() {
        let enrollment = BrowserEnrollment::begin("client-id").unwrap();
        assert!(enrollment.redirect_uri.starts_with("http://127.0.0.1:"));
        assert!(enrollment.authorize_url.contains("prompt=select%5Faccount"));
        assert!(!enrollment.authorize_url.contains("localhost"));
        let port = enrollment.listener.local_addr().unwrap().port();
        assert!(TcpListener::bind(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn callback_validates_state_before_the_code_is_exchanged() {
        let enrollment = BrowserEnrollment::begin("client-id").unwrap();
        let port = enrollment.listener.local_addr().unwrap().port();
        let (sent, received) = mpsc::channel();
        let worker = thread::spawn(move || {
            enrollment.complete_with(
                Duration::from_secs(2),
                |_, _| {
                    sent.send(()).unwrap();
                    unreachable!()
                },
                |_| unreachable!(),
            )
        });
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(b"GET /?state=wrong&code=secret HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            received.try_recv().is_err(),
            "a mismatched state spent the code"
        );
    }

    #[test]
    fn successful_callback_redeems_the_exact_redirect_and_verifier() {
        let enrollment = BrowserEnrollment::begin("client-id").unwrap();
        let state = enrollment.state.clone();
        let redirect = enrollment.redirect_uri.clone();
        let verifier = enrollment.verifier.clone();
        let port = enrollment.listener.local_addr().unwrap().port();
        let worker = thread::spawn(move || {
            enrollment.complete_with(
                Duration::from_secs(2),
                |url, fields| {
                    assert_eq!(
                        url,
                        "https://login.microsoftonline.com/common/oauth2/v2.0/token"
                    );
                    assert!(fields.contains(&("redirect_uri", redirect.as_str())));
                    assert!(fields.contains(&("code_verifier", verifier.as_str())));
                    assert!(fields.contains(&("code", "one-use-code")));
                    Ok(TokenResponse {
                        refresh_token: Some("refresh-secret".into()),
                        scope: Some(SCOPE.into()),
                        error: None,
                        error_description: None,
                    })
                },
                |refresh| {
                    assert_eq!(refresh.expose_for_storage(), "refresh-secret");
                    Ok(())
                },
            )
        });
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            stream,
            "GET /?state={state}&code=one-use-code HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        )
        .unwrap();
        let scope = worker.join().unwrap().unwrap();
        assert_eq!(scope.as_deref(), Some(SCOPE));
    }

    #[test]
    fn duplicate_state_is_refused() {
        let fields = parse_query("state=one&state=two&code=x").unwrap();
        assert!(one(&fields, "state").is_err());
    }

    #[test]
    fn secure_store_failure_is_reported_before_browser_success() {
        let enrollment = BrowserEnrollment::begin("client-id").unwrap();
        let state = enrollment.state.clone();
        let port = enrollment.listener.local_addr().unwrap().port();
        let worker = thread::spawn(move || {
            enrollment.complete_with(
                Duration::from_secs(2),
                |_, _| {
                    Ok(TokenResponse {
                        refresh_token: Some("refresh-secret".into()),
                        scope: None,
                        error: None,
                        error_description: None,
                    })
                },
                |_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "locked")),
            )
        });
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            stream,
            "GET /?state={state}&code=one-use-code HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(response.starts_with("HTTP/1.1 500"));
        assert!(!response.contains("completed and was stored"));
    }
}
