//! General CLI behavior (the exit-code contract): usage errors are
//! exit 2, `clap`'s default — exercised here rather than under
//! `validate.rs`/`pack_strip.rs` since neither subcommand owns them.

#![allow(clippy::unwrap_used)]

mod common;

#[test]
fn no_arguments_at_all_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = common::run(dir.path(), &[]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn an_unknown_subcommand_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let output = common::run(dir.path(), &["not-a-real-subcommand"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn top_level_help_documents_the_exit_code_contract() {
    let dir = tempfile::tempdir().unwrap();
    let output = common::run(dir.path(), &["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Exit codes"), "{stdout}");
    assert!(stdout.contains("usage error"), "{stdout}");
}
