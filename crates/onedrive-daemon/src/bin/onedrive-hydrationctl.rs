use onedrive_hydration_daemon::auth_state;
use onedrive_hydration_daemon::{control_request, runtime_socket};
use std::io;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage:\n  onedrive-hydrationctl [--socket <path>] status\n  \
         onedrive-hydrationctl [--socket <path>] evict <relative-path>"
    );
    std::process::exit(2)
}

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut socket = None;
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--socket" {
            socket = Some(PathBuf::from(args.next().unwrap_or_else(|| usage())));
        } else {
            positional.push(arg);
        }
    }
    let command = match positional.as_slice() {
        [verb] if verb == "status" => "status".to_owned(),
        [verb, path] if verb == "evict" && !path.is_empty() => format!("evict {path}"),
        _ => usage(),
    };
    let socket = socket
        .map(Ok)
        .unwrap_or_else(|| runtime_socket("onedrive-hydration.ctl"))?;
    let reply = control_request(&socket, &command)?;
    println!("{reply}");
    if command == "status" {
        // The sign-in state lives on the daemon's second socket, next to
        // the control socket, because the credential is product knowledge
        // the framework's status verb cannot answer. A daemon built before
        // that socket existed still answers everything above, so its
        // absence is reported with what actually happened rather than
        // being papered over or spelled as failure.
        match control_request(&auth_state::auth_socket(&socket), "status") {
            Ok(line) => println!("{line}"),
            Err(e) => println!(
                "sign-in: unknown — the sign-in state socket did not answer ({e}); \
                 the daemon is stopped or predates it"
            ),
        }
    }
    if reply.starts_with("error:") || reply.starts_with("unknown command:") {
        std::process::exit(1);
    }
    Ok(())
}
