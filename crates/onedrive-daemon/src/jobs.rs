//! User-requested availability jobs. Content reads run only in the ctl child.
use crate::control_request;
use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Coalescing notification bell so D-Bus desktop/job state publishes immediately
/// on job start, progress, cancellation, or control mutations rather than
/// waiting out a 2-second poll timer.
#[derive(Default)]
pub struct Wakeup {
    inner: (Mutex<bool>, Condvar),
}

impl Wakeup {
    pub fn notify(&self) {
        let (lock, cvar) = &self.inner;
        let mut pending = lock.lock().unwrap_or_else(|e| e.into_inner());
        *pending = true;
        cvar.notify_all();
    }

    pub fn wait_timeout(&self, timeout: Duration) {
        let (lock, cvar) = &self.inner;
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if *guard {
            let mut g = guard;
            *g = false;
            return;
        }
        let (mut g, _) = cvar
            .wait_timeout(guard, timeout)
            .unwrap_or_else(|e| e.into_inner());
        *g = false;
    }
}

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
        if selected
            .iter()
            .any(|sel| path.starts_with(sel) || sel.starts_with(&path))
        {
            return Err(io::Error::other(format!(
                "{relative}: local data needs review before changing availability. See Needs attention in OneDrive."
            )));
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
    wakeup: Arc<Wakeup>,
}

/// Remove entries whose ancestor directory is already in the sorted slice (`O(n)`).
fn prune_descendants_sorted(sorted: &mut Vec<PathBuf>) {
    let mut kept: Vec<PathBuf> = Vec::with_capacity(sorted.len());
    for candidate in sorted.drain(..) {
        if kept.last().is_some_and(|anc| candidate.starts_with(anc)) {
            continue;
        }
        kept.push(candidate);
    }
    *sorted = kept;
}

const BUSY_EIO_REPLY: &str = "error: Input/output error (os error 5)";
const BUSY_EIO_RETRIES: u32 = 5;

impl Jobs {
    pub fn with_wakeup(wakeup: Arc<Wakeup>) -> Self {
        Self {
            state: Mutex::new(Job::default()),
            cancel: AtomicBool::new(false),
            wakeup,
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, Job> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_running(&self) -> bool {
        self.lock_state().running
    }

    pub fn snapshot(&self) -> String {
        serde_json::to_string(&*self.lock_state()).unwrap_or_else(|_| "{}".to_owned())
    }

    pub fn cancel(&self) {
        // Release pairs with worker Acquire loads on `self.cancel`.
        self.cancel.store(true, Ordering::Release);
        self.wakeup.notify();
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
        prune_descendants_sorted(&mut selected);

        let mut state = self.lock_state();
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
        // Release ensures worker thread sees `cancel == false` on start.
        self.cancel.store(false, Ordering::Release);
        drop(state);
        self.wakeup.notify();

        let jobs = Arc::clone(self);
        std::thread::spawn(move || {
            if let Err(e) = jobs.run(&socket, &root, selected, &operation) {
                jobs.error(e.to_string());
            }
            let mut state = jobs.lock_state();
            // Acquire pairs with `cancel()` Release store.
            state.cancelled = jobs.cancel.load(Ordering::Acquire);
            state.running = false;
            drop(state);
            jobs.wakeup.notify();
        });
        Ok(())
    }

    fn error(&self, message: String) {
        let mut state = self.lock_state();
        if state.errors.len() < 100 {
            state.errors.push(message);
        }
        drop(state);
        self.wakeup.notify();
    }

    fn sleep_cancellable(&self, total: Duration) -> bool {
        let deadline = std::time::Instant::now() + total;
        while std::time::Instant::now() < deadline {
            if self.cancel.load(Ordering::Acquire) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        !self.cancel.load(Ordering::Acquire)
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
            if self.cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            let rel_path = path
                .strip_prefix(root)
                .map_err(|_| io::Error::other("Select items inside the OneDrive folder"))?;
            let relative = rel_path.to_string_lossy();
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
        self.lock_state().total = files.len();
        self.wakeup.notify();

        let ctl = std::env::current_exe()?.with_file_name("onedrive-hydrationctl");
        for path in files {
            if self.cancel.load(Ordering::Acquire) {
                break;
            }
            let Ok(rel_path) = path.strip_prefix(root) else {
                self.error("Skipped item outside OneDrive root".to_owned());
                self.lock_state().done += 1;
                self.wakeup.notify();
                continue;
            };
            let relative = rel_path.to_string_lossy().into_owned();
            if path.canonicalize().ok().as_ref() != Some(&path) || !path.is_file() {
                self.error(format!(
                    "{relative}: file moved or was replaced; select it again"
                ));
                self.lock_state().done += 1;
                self.wakeup.notify();
                continue;
            }
            self.lock_state().current = relative.clone();
            self.wakeup.notify();
            // Drawing or managing availability must not read resident files to
            // discover their state. This also avoids a whole-tree daemon scan
            // for every already-online-only item in a large folder.
            let dehydrated = {
                use std::os::unix::ffi::OsStrExt;
                let path = std::ffi::CString::new(path.as_os_str().as_bytes())
                    .map_err(io::Error::other)?;
                let mark = c"user.hydration.dehydrated";
                // SAFETY: `path` and `mark` are valid C strings; `(null, 0)` probes presence only.
                let found = unsafe {
                    libc::lgetxattr(path.as_ptr(), mark.as_ptr(), std::ptr::null_mut(), 0)
                };
                if found < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ENODATA) {
                    self.error(format!("{relative}: could not read availability metadata"));
                    self.lock_state().done += 1;
                    self.wakeup.notify();
                    continue;
                }
                found >= 0
            };
            if (operation == "free" && dehydrated) || (operation == "keep" && !dehydrated) {
                self.lock_state().done += 1;
                self.wakeup.notify();
                continue;
            }
            let reply = if operation == "free" {
                match control_request(socket, &format!("evict {relative}")) {
                    Ok(r) => r,
                    Err(e) => e.to_string(),
                }
            } else {
                // Retry transient single-flight busy `EIO` (measured when another
                // read is simultaneously hydrating on `hydrationd`), matching the
                // standalone Dolphin wrapper behavior.
                let mut attempt = 0u32;
                loop {
                    let out = std::process::Command::new(&ctl)
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
                        });
                    let rendered = match out {
                        Ok(r) => r,
                        Err(e) => break e.to_string(),
                    };
                    if rendered == BUSY_EIO_REPLY && attempt < BUSY_EIO_RETRIES {
                        attempt += 1;
                        if !self.sleep_cancellable(Duration::from_secs(1)) {
                            break rendered;
                        }
                        continue;
                    }
                    break rendered;
                }
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
                self.lock_state().bytes += bytes;
            } else {
                self.error(format!("{relative}: {reply}"));
            }
            self.lock_state().done += 1;
            self.wakeup.notify();
        }
        Ok(())
    }
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>, cancel: &AtomicBool) -> io::Result<()> {
    let mut dirs = vec![directory.to_owned()];
    while let Some(dir) = dirs.pop() {
        if cancel.load(Ordering::Acquire) {
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

        // Selecting a file *inside* a folder-level availability issue must also be blocked.
        let child_file = parent.join("sub.txt");
        std::fs::write(&child_file, "x").unwrap();
        let folder_issue =
            serde_json::json!({"issues": [{"path": "docs", "kind": "availability"}]});
        assert!(
            super::check_issues(dir, &[child_file.to_string_lossy().into()], &folder_issue)
                .is_err()
        );
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
