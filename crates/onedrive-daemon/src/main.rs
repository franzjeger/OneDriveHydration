use hydration_client::daemon_loop::{self, Config};
use hydration_graph::{DriveScope, GraphAccess, GraphHttp, TagSource};
use onedrive_hydration_daemon::{auth_config, discover_drive, runtime_socket, token_cache};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
         [--socket <path>]"
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
            let access = GraphAccess::with_token_cache(
                DriveScope::primary(profile.id),
                &mount,
                &args.state_dir,
                TagSource::QuickXor,
                cache,
            );
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
