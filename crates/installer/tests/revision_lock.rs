use std::path::PathBuf;
use std::process::{Command, Output};

const REV_A: &str = "85b4557322c85cb63978abcc385ac16e469aea82";
const REV_B: &str = "ba37078fe2b9b1976a9164a8571387879a9d7b63";

fn package(name: &str, revision: &str) -> String {
    format!(
        "[[package]]\nname = \"{name}\"\nversion = \"0.1.0\"\n\
         source = \"git+https://github.com/franzjeger/HydrationAPI?rev={revision}#{revision}\"\n\n"
    )
}

fn run(lock: &str) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let lock_file = dir.path().join("Cargo.lock");
    std::fs::write(&lock_file, lock).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    Command::new("bash")
        .arg(root.join("tools/hydration-api-rev.sh"))
        .arg(lock_file)
        .output()
        .unwrap()
}

fn required(revision: &str) -> String {
    [
        package("hydration-client", revision),
        package("hydration-graph", revision),
        package("hydration-protocol", revision),
    ]
    .concat()
}

#[test]
fn one_locked_revision_is_printed_as_a_full_commit() {
    let output = run(&required(REV_A));
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), REV_A);
}

#[test]
fn required_packages_at_different_revisions_are_refused() {
    let lock = [
        package("hydration-client", REV_A),
        package("hydration-graph", REV_B),
        package("hydration-protocol", REV_A),
    ]
    .concat();
    let output = run(&lock);
    assert!(!output.status.success(), "{:?}", output);
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("resolve to different commits"));
}

#[test]
fn an_additional_hydration_api_package_cannot_hide_a_second_revision() {
    let lock = required(REV_A) + &package("hydration-future", REV_B);
    let output = run(&lock);
    assert!(!output.status.success(), "{:?}", output);
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("not every HydrationAPI package resolves"));
}
