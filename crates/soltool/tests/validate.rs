//! `soltool validate` integration tests (including the exit-code
//! contract): spawns the real binary, which is how `main.rs` and the
//! library's dispatch get covered (`cargo llvm-cov` merges the spawned
//! binary's own coverage profile in automatically).

#![allow(clippy::unwrap_used)]

mod common;

#[test]
fn a_valid_theme_directory_exits_0_with_a_summary_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let theme_dir = dir.path().join("theme");
    common::write_minimal_theme(&theme_dir);

    let output = common::run(dir.path(), &["validate", theme_dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Minimal"), "{stdout}");
    assert!(stdout.contains("png"), "{stdout}");
    assert!(stdout.contains("1 back"), "{stdout}");
}

#[test]
fn a_theme_with_two_backs_pluralizes_the_summary() {
    let dir = tempfile::tempdir().unwrap();
    let theme_dir = dir.path().join("theme");
    common::write_two_back_theme(&theme_dir);

    let output = common::run(dir.path(), &["validate", theme_dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("2 backs"), "{stdout}");
}

#[test]
fn the_same_theme_zipped_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("theme.zip");
    std::fs::write(&zip_path, common::zip_minimal_theme()).unwrap();

    let output = common::run(dir.path(), &["validate", zip_path.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Minimal"), "{stdout}");
}

#[test]
fn a_theme_missing_one_face_exits_1_and_names_the_missing_face_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let theme_dir = dir.path().join("theme");
    common::write_minimal_theme(&theme_dir);
    std::fs::remove_file(theme_dir.join("cards").join("spades_01.png")).unwrap();

    let output = common::run(dir.path(), &["validate", theme_dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("spades_01"), "{stderr}");
}

#[test]
fn a_nonexistent_path_exits_1() {
    let dir = tempfile::tempdir().unwrap();

    let output = common::run(dir.path(), &["validate", "no/such/theme/here"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no/such/theme/here"), "{stderr}");
}
