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
    if reply.starts_with("error:") || reply.starts_with("unknown command:") {
        std::process::exit(1);
    }
    Ok(())
}
