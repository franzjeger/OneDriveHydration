use hydration_client::daemon_loop::{self, Config};
use hydration_graph::browser::Loopback;
use hydration_graph::{DriveScope, GraphAccess, GraphHttp, SharedTokenCache, TagSource};
use onedrive_hydration_daemon::auth_state::{self, CredentialHealth, PublisherOptions};
use onedrive_hydration_daemon::{
    auth_config, discover_drive, runtime_socket, token_cache, wait_for_secret_service,
    SECRET_SERVICE_WAIT,
};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

enum Command {
    Auth { browser: bool, no_open: bool },
    Run { mount: PathBuf },
}

struct Args {
    command: Command,
    state_dir: PathBuf,
    client_id: String,
    socket: PathBuf,
}

fn value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
    }
    None
}

fn required(name: &str) -> String {
    value(name).unwrap_or_else(|| {
        eprintln!("onedrive-hydration-daemon: missing {name}");
        usage();
    })
}

fn flag(name: &str) -> bool {
    std::env::args().skip(1).any(|arg| arg == name)
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  onedrive-hydration-daemon auth --state-dir <path> --client-id <uuid> \
         [--browser [--no-open]]\n  \
         onedrive-hydration-daemon run --mount <path> --state-dir <path> --client-id <uuid> \
         [--socket <path>]\n\n\
         --browser signs in through the system browser (authorization code + PKCE) \
         instead of the device code flow, which Conditional Access policies commonly \
         block; --no-open prints the sign-in URL instead of launching the browser"
    );
    std::process::exit(2)
}

fn parse() -> Args {
    let command = match std::env::args().nth(1).as_deref() {
        Some("auth") => Command::Auth {
            browser: flag("--browser"),
            no_open: flag("--no-open"),
        },
        Some("run") => Command::Run {
            mount: PathBuf::from(required("--mount")),
        },
        _ => usage(),
    };
    let state_dir = PathBuf::from(required("--state-dir"));
    let socket = value("--socket")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| runtime_socket("onedrive-hydration.sock"))
        .unwrap_or_else(|e| {
            eprintln!("onedrive-hydration-daemon: {e}");
            usage();
        });
    Args {
        command,
        state_dir,
        client_id: required("--client-id"),
        socket,
    }
}

fn auth_error(action: &'static str) -> impl FnOnce(hydration_graph::auth::AuthError) -> io::Error {
    move |_| io::Error::new(io::ErrorKind::PermissionDenied, action)
}

/// How long the browser sign-in waits for the redirect. The same bound the
/// enrollment script used; the human is the slow half, and five minutes is
/// several sign-ins' worth of typing a password and approving an MFA prompt.
const BROWSER_SIGN_IN_WAIT: Duration = Duration::from_secs(300);

/// The browser (authorization code + PKCE) sign-in, per the accepted
/// `docs/PKCE-ENROLLMENT-REVIEW.md`: a user-initiated, session-scoped act.
///
/// No `resume()` short-circuit on purpose, unlike the device code arm. This
/// command is also the re-enrollment path — the stored credential being
/// present says nothing about it working, and the one situation in which a
/// user runs it twice is a credential the service has rejected. The fresh
/// sign-in replaces the stored one; `prompt=select_account` (sent by
/// `begin_browser_code`) is what keeps a live SSO session from silently
/// re-enrolling the account that just failed.
fn browser_auth(cache: &SharedTokenCache, no_open: bool) -> io::Result<()> {
    // Bound before the URL exists, so the port in the URL is owned from the
    // moment anything could be redirected at it.
    let listener = Loopback::bind()?;
    let flow = cache
        .begin_browser_code(&listener.redirect_uri())
        .map_err(auth_error("browser enrollment could not be prepared"))?;

    // Printed before any launch attempt: whatever xdg-open does, the user can
    // always finish by hand from a browser that can reach this machine.
    println!("Sign in at:\n{}", flow.authorize_url());
    if no_open {
        println!("(--no-open: open the URL yourself)");
    } else {
        // Surfaced, never discarded: a silent xdg-open failure reads as a
        // hang, and the recovery — the printed URL — is already on screen.
        let opened = std::process::Command::new("xdg-open")
            .arg(flow.authorize_url())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match opened {
            Ok(status) if status.success() => println!("Opening the browser…"),
            Ok(status) => println!("xdg-open failed ({status}); use the URL above"),
            Err(e) => println!("could not run xdg-open ({e}); use the URL above"),
        }
    }
    println!("Waiting for the sign-in to finish…");

    let code = listener.wait(flow.state(), BROWSER_SIGN_IN_WAIT)?;
    cache
        .complete_browser_code(&flow, &code)
        .map_err(auth_error("browser enrollment did not complete"))?;
    println!("Sign-in completed and the rotated credential was stored.");

    // A daemon already running signed-out will not notice on its own. The
    // legacy-file path restarts itself — the daemon watches the state
    // directory for the enrollment file — but this flow writes straight into
    // Secret Service on purpose (no plaintext moment), and nothing watches
    // that. This process *is* the user's session, so it may do the restart
    // the file watcher used to: try-restart is a no-op when the unit is not
    // running, and the outcome is printed either way rather than assumed.
    let restarted = std::process::Command::new("systemctl")
        .args(["--user", "try-restart", "onedrive-hydration.service"])
        .status();
    match restarted {
        Ok(status) if status.success() => {
            println!("A running daemon (if any) was restarted onto the new sign-in.")
        }
        _ => println!(
            "If the daemon is running signed-out, restart it to pick up the new \
             sign-in: systemctl --user try-restart onedrive-hydration.service \
             (development invocations: stop and start the run command yourself)"
        ),
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let args = parse();
    if let Command::Run { .. } = &args.command {
        // At login the daemon races the credential store: the user manager
        // starts this unit before PAM has started the Secret Service
        // provider, and there is nothing the unit could order itself after
        // (measured; see wait_for_secret_service). Wait here, bounded, before
        // the first thing that reads the store — token_cache's migration
        // check. `auth` stays immediate: it is interactive, and a human at a
        // prompt is better served by the error than by a silent minute.
        wait_for_secret_service(SECRET_SERVICE_WAIT)?;
    }
    let cache = token_cache(auth_config(args.client_id), &args.state_dir)?;
    match args.command {
        Command::Auth { browser, no_open } => {
            if browser {
                return browser_auth(&cache, no_open);
            }
            if cache.resume()? {
                println!("Already signed in.");
                return Ok(());
            }
            let code = cache
                .begin_device_code()
                .map_err(auth_error("device-code enrollment could not be started"))?;
            println!("Open {}", code.verification_uri());
            println!("Enter code: {}", code.user_code());
            cache
                .complete_device_code(&code)
                .map_err(auth_error("device-code enrollment did not complete"))?;
            println!("Sign-in completed and the rotated credential was stored.");
            Ok(())
        }
        Command::Run { mount } => {
            if !mount.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "the sync mount directory does not exist",
                ));
            }
            if !cache.resume()? {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "not signed in; run the auth command first",
                ));
            }
            let profile = discover_drive(&mut GraphHttp::new(Arc::clone(&cache)))?;
            eprintln!(
                "onedrive-hydration-daemon: drive={} type={}",
                profile.id.as_str(),
                profile.drive_type
            );
            // The publisher's clone, taken before the cache moves into the
            // provider roles: the same shared cache all three roles refresh
            // through is the one whose conclusions get published.
            let observed = Arc::clone(&cache);
            let access = GraphAccess::with_token_cache(
                DriveScope::primary(profile.id),
                &mount,
                &args.state_dir,
                TagSource::QuickXor,
                cache,
            );

            // The sign-in state socket, next to daemon_loop's control socket.
            // Product-owned because the credential is product knowledge: the
            // run loop sees only opaque provider roles. A bind failure is
            // announced and survived — the daemon can still sync without the
            // surface, and the D-Bus side then answers "unknown", which is
            // true — mirroring how daemon_loop treats its control socket.
            let auth_socket = auth_state::auth_socket(&args.socket);
            let enrollment = args.state_dir.join("refresh-token");
            std::thread::spawn(move || {
                let served = auth_state::serve(
                    &auth_socket,
                    PublisherOptions {
                        sample_interval: auth_state::SAMPLE_INTERVAL,
                        enrollment: Some(enrollment),
                    },
                    &mut || CredentialHealth {
                        signed_in: observed.is_signed_in(),
                        store_error: observed.last_store_error(),
                    },
                    // Adopting an enrollment is a restart on purpose: the
                    // startup path is the only code that reads credential
                    // bytes, and it re-runs drive discovery, so a sign-in
                    // that names a different account is renegotiated rather
                    // than spliced under a running sync. Nonzero, because
                    // the unit restarts on failure only.
                    &mut || std::process::exit(1),
                    &mut || true,
                );
                if let Err(e) = served {
                    eprintln!(
                        "onedrive-hydration-daemon: the sign-in state socket could not be \
                         served: {e}"
                    );
                }
            });

            daemon_loop::run(
                Config {
                    mount,
                    socket: args.socket,
                    debounce: Duration::from_secs(900),
                },
                access,
            )
        }
    }
}
