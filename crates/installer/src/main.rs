//! CLI for the validated installer. All judgment lives in the library; this
//! file only parses arguments, gathers the real measurements, and prints.

use onedrive_hydration_install::plan::{self, ExecMode, Options, Outcome};
use onedrive_hydration_install::probes;
use onedrive_hydration_install::units::{Templates, Tray};
use onedrive_hydration_install::Facts;
use std::io;
use std::path::{Path, PathBuf};

fn usage() -> ! {
    eprintln!(
        "usage:\n  onedrive-hydration-install install --user <name> --mount <path> \
         --client-id <uuid>\n      [--bin-dir <dir>] [--prefix <dir>] [--dry-run] [--force] \
         [--consent-fstab]\n      [--tray sni|plasmoid|none]\n  \
         onedrive-hydration-install uninstall --user <name> --mount <path>\n      \
         [--prefix <dir>] [--dry-run] [--and-unmount]\n  onedrive-hydration-install render \
         --user <name> --mount <path> --client-id <uuid>\n      [--bin-dir <dir>] \
         [--tray sni|plasmoid|none]\n\n\
         render prints the units install would write — for review, and for diffing an\n\
         already-deployed set against what this version generates. It validates nothing\n\
         except the generated text itself and writes no files.\n\n\
         Validates the machine and either writes concrete systemd units for exactly\n\
         this user, uid, sync root and runtime socket — or refuses, saying why.\n\n\
         It will never: create or delete the btrfs subvolume (the command is printed\n\
         instead); touch /etc/fstab without --consent-fstab, and never without noauto;\n\
         enroll credentials or invent a client id; install the Plasma applet.\n\n\
         --tray picks which surface draws the tray icon. There is no auto: the applet\n\
         is a tray entry in its own right, so installing both shows two identical\n\
         icons, and which desktop the user logs into is not a fact at install time.\n\
         sni      onedrive-hydration-tray.service — any StatusNotifierWatcher\n\
         plasmoid the Plasma applet (packaging/plasmoid/install-plasmoid.sh, which\n\
                  this tool does not run); a tray unit from an earlier install is\n\
                  removed\n\
         none     no tray; onedrive-hydrationctl status is the surface\n\
         Left out, it defaults to sni — unless the applet is already installed for\n\
         that user, which is refused until one of the three is named.\n\n\
         --prefix <dir> rehearses against a scratch root: files land under the prefix\n\
         and no command is executed. --dry-run prints everything and writes nothing."
    );
    std::process::exit(2)
}

enum Mode {
    Install,
    Uninstall,
    Render,
}

struct Cli {
    mode: Mode,
    user: String,
    mount: PathBuf,
    client_id: String,
    bin_dir: PathBuf,
    opts: Options,
    and_unmount: bool,
}

fn parse() -> Cli {
    let mut args = std::env::args().skip(1);
    let mode = match args.next().as_deref() {
        Some("install") => Mode::Install,
        Some("uninstall") => Mode::Uninstall,
        Some("render") => Mode::Render,
        _ => usage(),
    };
    let (mut user, mut mount, mut client_id, mut bin_dir) = (None, None, None, None);
    let mut opts = Options {
        prefix: PathBuf::from("/"),
        dry_run: false,
        force: false,
        consent_fstab: false,
        tray: None,
    };
    let mut and_unmount = false;
    fn take(args: &mut impl Iterator<Item = String>) -> String {
        args.next().unwrap_or_else(|| usage())
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--user" => user = Some(take(&mut args)),
            "--mount" => mount = Some(PathBuf::from(take(&mut args))),
            "--client-id" => client_id = Some(take(&mut args)),
            "--bin-dir" => bin_dir = Some(PathBuf::from(take(&mut args))),
            "--prefix" => opts.prefix = PathBuf::from(take(&mut args)),
            "--dry-run" => opts.dry_run = true,
            "--force" => opts.force = true,
            "--consent-fstab" => opts.consent_fstab = true,
            "--tray" => {
                let v = take(&mut args);
                // A specific complaint rather than the whole usage block: the
                // one thing someone reaching for --tray is likely to try is
                // "auto", and the reason there isn't one is the answer they
                // need.
                opts.tray = Some(Tray::parse(&v).unwrap_or_else(|| {
                    eprintln!(
                        "--tray {v:?}: expected sni, plasmoid or none. There is no auto — \
                         which desktop this user logs into is not a fact this installer \
                         can measure, and guessing it decides which tray surface gets \
                         installed"
                    );
                    std::process::exit(2)
                }));
            }
            "--and-unmount" => and_unmount = true,
            _ => usage(),
        }
    }
    let (Some(user), Some(mount)) = (user, mount) else {
        usage()
    };
    if matches!(mode, Mode::Install | Mode::Render) && client_id.is_none() {
        // Required, not defaulted: an id this tool invented would be an id
        // nobody registered, and enrollment would fail against it.
        usage()
    }
    Cli {
        mode,
        user,
        mount,
        client_id: client_id.unwrap_or_default(),
        bin_dir: bin_dir.unwrap_or_else(|| PathBuf::from("/usr/local/bin")),
        opts,
        and_unmount,
    }
}

fn print_checks(checks: &[plan::Check]) {
    for c in checks {
        match &c.outcome {
            Outcome::Pass(m) => println!("check: ok — [{}] {m}", c.name),
            Outcome::Caveat(m) => println!("check: note — [{}] {m}", c.name),
            Outcome::Refuse(m) => println!("REFUSED [{}]: {m}", c.name),
        }
    }
}

fn main() -> io::Result<()> {
    let cli = parse();
    let facts = match Facts::resolve(
        &cli.user,
        cli.mount.clone(),
        cli.client_id.clone(),
        cli.bin_dir.clone(),
    ) {
        Ok(f) => f,
        Err(e) => {
            println!("REFUSED [user]: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "facts: user={} uid={} home={}\n       mount={} socket={} (derived from the uid, \
         never assumed)",
        facts.user,
        facts.uid,
        facts.home.display(),
        facts.mount.display(),
        facts.socket.display()
    );

    if let Mode::Render = cli.mode {
        // Review and drift-detection: print exactly what install would write.
        // The one validation that still applies is the one about this text
        // itself — a reviewer must not be shown a unit the installer would
        // refuse.
        //
        // render looks at no machine, so it cannot see whether the applet is
        // installed and cannot reach install's refusal. It renders for the
        // surface it was told, defaults to the same sni that install defaults
        // to, and says which — a set diffed against a deployment's units has
        // to be the set for the same tray surface or the diff is noise.
        let tray = cli.opts.tray.unwrap_or(Tray::Sni);
        println!("tray surface: {} (--tray)", tray.as_str());
        let rendered =
            onedrive_hydration_install::units::render(&Templates::default(), &facts, tray);
        for unit in rendered.all() {
            println!("# ==> {} <==", unit.name);
            print!("{}", unit.text);
            println!();
        }
        let mut poisoned = false;
        for unit in rendered.all() {
            if !onedrive_hydration_install::units::must_share_host_namespace(&unit.name) {
                continue;
            }
            for (line, directive) in
                onedrive_hydration_install::units::namespace_directives(&unit.text)
            {
                eprintln!(
                    "REFUSED [unit-text]: {}:{line} carries {directive}=, which would give \
                     the helper a private mount namespace; install would refuse this",
                    unit.name
                );
                poisoned = true;
            }
        }
        std::process::exit(if poisoned { 1 } else { 0 });
    }

    let planned = match cli.mode {
        Mode::Install => {
            let observed = plan::observe(&facts, &cli.opts.prefix)?;
            plan::install(&facts, &Templates::default(), &observed, &cli.opts)
        }
        _ => {
            let table = std::fs::read_to_string("/proc/self/mountinfo")?;
            let rows = probes::parse_mountinfo(&table);
            let mounted = probes::find_mount(&rows, &facts.mount).is_some();
            plan::uninstall(&facts, mounted, cli.and_unmount, &cli.opts)
        }
    };

    print_checks(&planned.checks);
    let Some(actions) = planned.actions else {
        println!("nothing was written.");
        std::process::exit(1);
    };

    if cli.opts.prefix != Path::new("/") {
        println!(
            "rehearsal: files go under {}, commands are printed, never run",
            cli.opts.prefix.display()
        );
    }
    let (log, result) = plan::execute(&actions, ExecMode::from(&cli.opts));
    for line in log {
        println!("  {line}");
    }
    result
}
