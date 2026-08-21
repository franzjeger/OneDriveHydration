//! The tag artifact must contain the same payload the installer validates.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn release_assembly_contains_every_runtime_binary_and_desktop_surface() {
    let script = std::fs::read_to_string(root().join("tools/assemble-release.sh")).unwrap();
    for required in [
        "hydrationd",
        "onedrive-hydration-daemon",
        "onedrive-hydrationctl",
        "onedrive-hydration-dbus",
        "onedrive-hydration-tray",
        "onedrive-hydration-install",
        "product/docs",
        "product/packaging",
        "MANIFEST.sha256",
        "HYDRATION_API_REVISION",
    ] {
        assert!(
            script.contains(required),
            "release payload omits {required}"
        );
    }
    assert!(
        !script.contains("payload/libexec"),
        "the installer looks for hydrationd in --bin-dir, not libexec"
    );
    assert!(
        !script.contains("pkce-enroll.py"),
        "the release must not ship the obsolete plaintext-token enrollment helper"
    );
    assert!(
        script.contains("basename \"$output\""),
        "the published checksum must be portable, not contain a CI-only absolute path"
    );
}

#[test]
fn a_tag_publishes_the_archive_instead_of_only_retaining_a_ci_artifact() {
    let workflow = std::fs::read_to_string(root().join(".github/workflows/release.yml")).unwrap();
    assert!(workflow.contains("gh release create"));
    assert!(workflow.contains("contents: write"));
    assert!(workflow.contains("--verify-tag"));
}
