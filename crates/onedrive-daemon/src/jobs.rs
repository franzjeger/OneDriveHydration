//! User-requested availability jobs. Content reads run only in the ctl child.
use crate::control_request;
use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Metadata-only check before an operation can read or evict ambiguous bytes.
pub fn check_issues(root: &Path, paths: &[String], state: &serde_json::Value) -> io::Result<()> {
    let selected: Vec<_> = paths
        .iter()
        .map(|p| Path::new(p).canonicalize())
        .collect::<io::Result<_>>()?;
    let root = root.canonicalize()?;
    for issue in state["issues"].as_array().into_iter().flatten() {
        if issue["kind"] != "availability" {
            continue;
        }
        let Some(relative) = issue["path"].as_str() else {
            continue;
        };
        let path = root.join(relative);
        if selected.iter().any(|selected| path.starts_with(selected)) {
            return Err(io::Error::other(format!("{relative}: local data needs review before changing availability. See Needs attention in OneDrive.")));
        }
    }
    Ok(())
}

#[derive(Default, Clone, Serialize)]
pub struct Job {
    pub id: u64,
    pub running: bool,
    pub operation: String,
    pub current: String,
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub cancelled: bool,
    pub errors: Vec<String>,
}

#[derive(Default)]
pub struct Jobs {
    state: Mutex<Job>,
    cancel: AtomicBool,
}

impl Jobs {
    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap().running
    }
    pub fn snapshot(&self) -> String {
        serde_json::to_string(&*self.state.lock().unwrap()).unwrap()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    pub fn start(
        self: &Arc<Self>,
        socket: PathBuf,
        root: PathBuf,
        paths: Vec<String>,
        operation: String,
    ) -> io::Result<()> {
        if !matches!(operation.as_str(), "keep" | "free") || paths.is_empty() || paths.len() > 1000
        {
            return Err(io::Error::other(
                "Choose files or folders and a valid availability action",
            ));
        }
        let root = root.canonicalize()?;
        let mut selected = Vec::new();
        for path in paths {
            let path = PathBuf::from(path).canonicalize()?;
            if path == root
                || !path.starts_with(&root)
                || path.to_string_lossy().contains(['\n', '\r'])
            {
                return Err(io::Error::other("Select items inside the OneDrive folder"));
            }
            selected.push(path);
        }
        selected.sort();
        selected.dedup();
        let all = selected.clone();
        selected.retain(|p| {
            !all.iter()
                .any(|parent| p != parent && p.starts_with(parent))
        });
        let mut state = self.state.lock().unwrap();
        if state.running {
            return Err(io::Error::other(
                "An availability job is already running; finish or cancel it first",
            ));
        }
        let id = state.id.saturating_add(1);
        *state = Job {
            id,
            running: true,
            operation: operation.clone(),
            ..Job::default()
        };
        self.cancel.store(false, Ordering::SeqCst);
        drop(state);
        let jobs = Arc::clone(self);
        std::thread::spawn(move || {
            if let Err(e) = jobs.run(&socket, &root, selected, &operation) {
                jobs.error(e.to_string());
            }
            let mut state = jobs.state.lock().unwrap();
            state.cancelled = jobs.cancel.load(Ordering::SeqCst);
            state.running = false;
        });
        Ok(())
    }
    fn error(&self, message: String) {
        let mut state = self.state.lock().unwrap();
        if state.errors.len() < 100 {
            state.errors.push(message);
        }
    }
    fn run(
        &self,
        socket: &Path,
        root: &Path,
        selected: Vec<PathBuf>,
        operation: &str,
    ) -> io::Result<()> {
        let mut files = Vec::new();
        for path in selected {
            if self.cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            let relative = path.strip_prefix(root).unwrap().to_string_lossy();
            let command = format!(
                "{} {relative}",
                if operation == "keep" { "pin" } else { "unpin" }
            );
            let reply = control_request(socket, &command)?;
            let expected = if operation == "keep" {
                "pinned"
            } else {
                "unpinned"
            };
            if reply.trim() != expected {
                self.error(format!("{relative}: {reply}"));
                continue;
            }
            if path.is_dir() {
                if operation == "keep" {
                    let reply = control_request(socket, &format!("pending {relative}"))?;
                    if reply.starts_with("error:") {
                        self.error(format!("{relative}: {reply}"));
                        continue;
                    }
                    for child in reply.lines().filter(|p| !p.is_empty()) {
                        let child = root.join(child).canonicalize()?;
                        if !child.starts_with(&path) {
                            return Err(io::Error::other(
                                "Daemon returned an item outside the selected folder",
                            ));
                        }
                        files.push(child);
                    }
                } else {
                    collect(&path, &mut files, &self.cancel)?;
                }
            } else {
                files.push(path);
            }
        }
        files.sort();
        files.dedup();
        self.state.lock().unwrap().total = files.len();
        let ctl = std::env::current_exe()?.with_file_name("onedrive-hydrationctl");
        for path in files {
            if self.cancel.load(Ordering::SeqCst) {
                break;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.canonicalize().ok().as_ref() != Some(&path) || !path.is_file() {
                self.error(format!(
                    "{relative}: file moved or was replaced; select it again"
                ));
                self.state.lock().unwrap().done += 1;
                continue;
            }
            self.state.lock().unwrap().current = relative.clone();
            // Drawing or managing availability must not read resident files to
            // discover their state. This also avoids a whole-tree daemon scan
            // for every already-online-only item in a large folder.
            let dehydrated = {
                use std::os::unix::ffi::OsStrExt;
                let path = std::ffi::CString::new(path.as_os_str().as_bytes())
                    .map_err(io::Error::other)?;
                let mark = c"user.hydration.dehydrated";
                let found = unsafe {
                    libc::lgetxattr(path.as_ptr(), mark.as_ptr(), std::ptr::null_mut(), 0)
                };
                if found < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ENODATA) {
                    self.error(format!("{relative}: could not read availability metadata"));
                    self.state.lock().unwrap().done += 1;
                    continue;
                }
                found >= 0
            };
            if (operation == "free" && dehydrated) || (operation == "keep" && !dehydrated) {
                self.state.lock().unwrap().done += 1;
                continue;
            }
            let result = if operation == "free" {
                control_request(socket, &format!("evict {relative}"))
            } else {
                std::process::Command::new(&ctl)
                    .arg("hydrate")
                    .arg(&path)
                    .output()
                    .map(|out| {
                        String::from_utf8_lossy(if out.stdout.is_empty() {
                            &out.stderr
                        } else {
                            &out.stdout
                        })
                        .trim()
                        .to_owned()
                    })
            };
            let reply = match result {
                Ok(reply) => reply,
                Err(error) => error.to_string(),
            };
            let prefix = if operation == "keep" {
                "hydrated "
            } else {
                "reclaimed "
            };
            let bytes = reply
                .trim()
                .strip_prefix(prefix)
                .and_then(|s| s.strip_suffix(" bytes"))
                .and_then(|s| s.parse::<u64>().ok());
            if operation == "free" && reply.trim() == "kept: AlreadyDehydrated" {
                // A concurrent eviction reached the same safe end state.
            } else if let Some(bytes) = bytes {
                self.state.lock().unwrap().bytes += bytes;
            } else {
                self.error(format!("{relative}: {reply}"));
            }
            self.state.lock().unwrap().done += 1;
        }
        Ok(())
    }
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>, cancel: &AtomicBool) -> io::Result<()> {
    let mut dirs = vec![directory.to_owned()];
    while let Some(dir) = dirs.pop() {
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".hydration-") || name.starts_with(".onedrive-") {
                continue;
            }
            if name.contains(['\n', '\r']) {
                return Err(io::Error::other(
                    "A filename contains an unsupported line break",
                ));
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                dirs.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
            }
            if files.len() + dirs.len() > 1_000_000 {
                return Err(io::Error::other(
                    "Select a smaller folder for this operation",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn ambiguous_data_blocks_parent_actions_without_reading_content() {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path();
        let parent = dir.join("docs");
        std::fs::create_dir_all(&parent).unwrap();
        let state = serde_json::json!({"issues": [{"path": "docs/a.txt", "kind": "availability"}]});
        assert!(super::check_issues(dir, &[parent.to_string_lossy().into()], &state).is_err());
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        assert!(super::check_issues(dir, &[other.to_string_lossy().into()], &state).is_ok());
    }

    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::time::{Duration, Instant};

    #[test]
    fn mixed_selection_is_deduplicated_and_errors_are_preserved() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("drive");
        let folder = root.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.txt"), "a").unwrap();
        std::fs::write(folder.join("b.txt"), "b").unwrap();
        let socket = scratch.path().join("ctl");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let mut commands = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut command = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut command)
                    .unwrap();
                let reply = if command.starts_with("unpin ") {
                    "unpinned"
                } else if command.contains("a.txt") {
                    "reclaimed 4096 bytes"
                } else {
                    "kept: Unsynced"
                };
                writeln!(stream, "{reply}").unwrap();
                commands.push(command);
            }
            commands
        });
        let jobs = Arc::new(Jobs::default());
        jobs.start(
            socket,
            root,
            vec![
                folder.to_string_lossy().into_owned(),
                folder.join("a.txt").to_string_lossy().into_owned(),
            ],
            "free".into(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while jobs.state.lock().unwrap().running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let state = jobs.state.lock().unwrap().clone();
        assert!(!state.running);
        assert_eq!(state.total, 2);
        assert_eq!(state.done, 2);
        assert_eq!(state.bytes, 4096);
        assert_eq!(state.errors, vec!["folder/b.txt: kept: Unsynced"]);
        assert_eq!(server.join().unwrap().len(), 3);
    }

    #[test]
    fn outside_selection_is_refused_before_starting_a_job() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("drive");
        std::fs::create_dir(&root).unwrap();
        let outside = scratch.path().join("outside");
        std::fs::write(&outside, "scratch").unwrap();
        let jobs = Arc::new(Jobs::default());
        assert!(jobs
            .start(
                scratch.path().join("missing-socket"),
                root,
                vec![outside.to_string_lossy().into_owned()],
                "keep".into()
            )
            .is_err());
        assert!(!jobs.state.lock().unwrap().running);
    }
}
