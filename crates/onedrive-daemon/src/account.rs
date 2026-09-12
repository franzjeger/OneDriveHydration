//! Display metadata from the daemon's existing authenticated Graph session.
use hydration_graph::{GraphHttp, Method, Request, SharedTokenCache, Transport};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;

pub fn sample(cache: SharedTokenCache, mount: &Path) -> io::Result<serde_json::Value> {
    let reply = GraphHttp::new(cache).send(&Request::new(
        Method::Get,
        "https://graph.microsoft.com/v1.0/me/drive?$select=id,driveType,name,owner,quota,webUrl",
    ))?;
    if !(200..300).contains(&reply.status) {
        return Err(io::Error::other(format!(
            "Account details returned HTTP {}",
            reply.status
        )));
    }
    let mut value: serde_json::Value = serde_json::from_slice(&reply.body)?;
    let map = value
        .as_object_mut()
        .ok_or_else(|| io::Error::other("Invalid account details"))?;
    map.retain(|key, _| {
        ["id", "driveType", "name", "owner", "quota", "webUrl"].contains(&key.as_str())
    });
    map.insert(
        "updated_at".into(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .into(),
    );
    if let Ok(path) = std::ffi::CString::new(mount.as_os_str().as_bytes()) {
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // statvfs inspects filesystem capacity; it never opens placeholder data.
        if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } == 0 {
            let stats = unsafe { stats.assume_init() };
            map.insert(
                "disk_available".into(),
                stats.f_bavail.saturating_mul(stats.f_frsize).into(),
            );
        }
    }
    Ok(value)
}

pub fn serve(cache: SharedTokenCache, mount: std::path::PathBuf, path: std::path::PathBuf) {
    loop {
        let update = || -> io::Result<()> {
            let profile = sample(Arc::clone(&cache), &mount)?;
            let tmp = path.with_extension("tmp");
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(&serde_json::to_vec(&profile)?)?;
            std::fs::rename(tmp, &path)
        };
        if let Err(error) = update() {
            eprintln!("onedrive-hydration: could not refresh account details: {error}");
        }
        std::thread::sleep(std::time::Duration::from_secs(300));
    }
}
