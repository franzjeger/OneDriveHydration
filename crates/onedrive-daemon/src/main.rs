use hydration_client::daemon_loop::{self, Config};
use hydration_graph::auth::AuthConfig;
use hydration_graph::{DriveId, DriveScope, GraphAccess, TagSource};
use std::io;
use std::path::PathBuf;
use std::time::Duration;

struct Args {
    mount: PathBuf,
    state_dir: PathBuf,
    drive_id: DriveId,
    client_id: String,
    credential: PathBuf,
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
        "usage: onedrive-hydration-daemon --mount <path> --state-dir <path> \
         --drive-id <id> --client-id <uuid> [--credential <path>] [--socket <path>]"
    );
    std::process::exit(2)
}

fn parse() -> io::Result<Args> {
    if std::env::args().any(|arg| matches!(arg.as_str(), "-h" | "--help")) {
        usage();
    }
    let mount = PathBuf::from(required("--mount"));
    let state_dir = PathBuf::from(required("--state-dir"));
    let drive_id = DriveId::parse(&required("--drive-id"))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid --drive-id"))?;
    let credential = value("--credential")
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir.join("refresh-token"));
    let socket = value("--socket").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("onedrive-hydration.sock")
    });
    Ok(Args {
        mount,
        state_dir,
        drive_id,
        client_id: required("--client-id"),
        credential,
        socket,
    })
}

fn main() -> io::Result<()> {
    let args = parse()?;
    if !args.mount.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the sync mount directory does not exist",
        ));
    }
    let auth =
        AuthConfig::public_client(args.client_id).with_scopes(["Files.ReadWrite.All", "User.Read"]);
    let access = GraphAccess::new(
        DriveScope::primary(args.drive_id),
        &args.mount,
        &args.state_dir,
        args.credential,
        auth,
        TagSource::CTag,
    );
    daemon_loop::run(
        Config {
            mount: args.mount,
            socket: args.socket,
            debounce: Duration::from_secs(900),
        },
        access,
    )
}
