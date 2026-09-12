//! Exercise generated actions against a fake daemon, never a live sync root.
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn folder_action_preserves_failures_and_partial_success() {
    for (reply, success, expected) in [
        ("error: daemon unavailable", false, "daemon unavailable"),
        ("kept: Unsynced", false, "kept: Unsynced"),
        (
            "kept: AlreadyDehydrated",
            true,
            "had no local space to reclaim",
        ),
        ("reclaimed 4096 bytes", true, "Freed 4.0KiB"),
        ("reclaimed invalid bytes", false, "Invalid response"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("drive");
        let folder = root.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("example.txt"), "scratch").unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        executable(&bin.join("ctl"), "#!/bin/sh\nif [ \"$1\" = unpin ]; then echo unpinned; else printf '%s\\n' \"$TEST_REPLY\"; fi\n");
        executable(&bin.join("kdialog"), "#!/bin/sh\ncase \"$*\" in *--progressbar*) exit 0;; esac\nprintf '%s\\n' \"$@\" > \"$TEST_UI\"\n");
        executable(&bin.join("dbus-send"), "#!/bin/sh\nexit 0\n");
        let source = include_str!("../../../packaging/dolphin/free-up-space-folder.sh.in")
            .replace("@MOUNT@", root.to_str().unwrap())
            .replace("@CTL@", bin.join("ctl").to_str().unwrap());
        let script = dir.path().join("action.sh");
        std::fs::write(&script, source).unwrap();
        let ui = dir.path().join("ui");
        let result = Command::new("sh")
            .arg(&script)
            .arg(&folder)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("TEST_REPLY", reply)
            .env("TEST_UI", &ui)
            .output()
            .unwrap();
        let message = std::fs::read_to_string(ui).unwrap();
        assert_eq!(result.status.success(), success, "{message}");
        assert!(message.contains(expected), "{message}");
        assert_eq!(message.contains("--sorry"), !success, "{message}");
        assert_eq!(
            std::fs::read_to_string(folder.join("example.txt")).unwrap(),
            "scratch"
        );
    }
}
