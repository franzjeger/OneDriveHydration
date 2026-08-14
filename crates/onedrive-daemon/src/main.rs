use hydration_client::daemon_loop::{self, Config};
use hydration_graph::{DriveScope, GraphAccess, GraphHttp, TagSource};
use onedrive_hydration_daemon::auth_state::{self, CredentialHealth, PublisherOptions};
use onedrive_hydration_daemon::{
    auth_config, discover_drive, runtime_socket, token_cache, wait_for_secret_service,
    SECRET_SERVICE_WAIT,
};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

enum Command {
    Auth,
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

/// A presence flag with no value, e.g. `--autoevict`.
fn flag(name: &str) -> bool {
    std::env::args().skip(1).any(|a| a == name)
}

fn required(name: &str) -> String {
    value(name).unwrap_or_else(|| {
        eprintln!("onedrive-hydration-daemon: missing {name}");
        usage();
    })
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  onedrive-hydration-daemon auth --state-dir <path> --client-id <uuid>\n  \
         onedrive-hydration-daemon run --mount <path> --state-dir <path> --client-id <uuid> \
         [--socket <path>] [--autoevict]"
    );
    std::process::exit(2)
}

fn parse() -> Args {
    let command = match std::env::args().nth(1).as_deref() {
        Some("auth") => Command::Auth,
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
        Command::Auth => {
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
            // cTag, and the choice is not free — it is the lesser of two costs.
            //
            // A tag is used for three things: telling whether an object changed,
            // verifying downloaded content, and guarding an update as an
            // `if-match`. QuickXorHash is a hash of the object, so it does the
            // first two and cannot do the third: Graph accepts no hash as a
            // precondition, and `GraphSink::precondition` refuses rather than
            // write blind. Measured on a live account on 2026-08-13, that meant
            // *every* update to a file that already existed was refused. Six
            // edits sat on the machine for hours. A sync client that cannot send
            // a change to an existing document is not one.
            //
            // What it costs: `GraphProvider::fetch` verifies a `qx:` tag against
            // the bytes it downloaded, and with cTags there is no hash to check,
            // so that verification does not run. Corruption in transit is left to
            // TLS and to the service, which is where every other client leaves
            // it.
            //
            // Only consulted when nothing is pinned yet. Once a drive has a
            // persisted tree, `GraphAccess` follows what that says — every tag
            // already written is of that shape, and `delta::is_current` compares
            // them byte for byte.
            let access = GraphAccess::with_token_cache(
                DriveScope::primary(profile.id),
                &mount,
                &args.state_dir,
                TagSource::CTag,
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
                    // The framework's constant, not a number of our own. A
                    // quarter of an hour was what this said before, with no
                    // reasoning anywhere, and it meant a file created in the
                    // sync folder did not reach the cloud until long after its
                    // owner had concluded the client was broken.
                    debounce: hydration_client::upload::QUIET_PERIOD,
                    // Off unless `--autoevict` is passed: auto-freeing local
                    // space is opt-in. When on, the framework's default
                    // disk-pressure policy — dehydrate the least-recently-
                    // acquired unpinned files below a low-water mark, honoring
                    // the pin. Off is off: with `None` the framework spawns no
                    // eviction thread at all.
                    eviction: flag("--autoevict")
                        .then(hydration_client::evict_policy::EvictionConfig::default_pressure),
                },
                access,
            )
        }
    }
}
