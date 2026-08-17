//! The README's `soltool` section, checked against `clap`'s own `--help`.
//!
//! Documentation drifts because nothing checks it. Exactly one class of
//! drift here is mechanically gateable: a flag that exists in the CLI and
//! not in the README. This test closes that class — everything else in the
//! README stays a human's job, and is called that rather than promised.

#![allow(clippy::unwrap_used, clippy::expect_used)] // Test fixtures: a broken fixture must abort the suite loudly.

use std::process::Command;

/// Every long flag `soltool <subcommand> --help` advertises, minus the ones
/// `clap` supplies for itself.
fn documented_flags(args: &[&str]) -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_soltool"))
        .args(args)
        .arg("--help")
        .output()
        .expect("running soltool --help");
    assert!(
        output.status.success(),
        "soltool {args:?} --help exited {:?}",
        output.status
    );
    let help = String::from_utf8(output.stdout).expect("--help output is UTF-8");
    // Only the `Options:` block: the prose above it may *name* a flag while
    // describing something else (the top-level help mentions
    // `extract --animate` when explaining exit codes), and that is a
    // sentence, not an advertised flag.
    let options = help
        .split_once("Options:")
        .map_or("", |(_, options)| options);

    let mut flags: Vec<String> = options
        .split_whitespace()
        .filter_map(|word| {
            // Trim the punctuation `clap` wraps flags in: `--flag,`
            // `[--flag]`, `<--flag>`.
            let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            word.strip_prefix("--").map(|_| word.to_owned())
        })
        // `clap` adds these itself; the README documents the tool, not clap.
        .filter(|flag| flag != "--help" && flag != "--version")
        .collect();
    flags.sort();
    flags.dedup();
    flags
}

/// The README's `soltool` section: from its heading to the next one.
fn readme_soltool_section() -> String {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))
        .expect("reading README.md");
    let start = readme
        .find("## `soltool`")
        .expect("README has a soltool section");
    let rest = &readme[start + "## `soltool`".len()..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    rest[..end].to_owned()
}

/// The subcommands whose flags the README's usage lines spell out.
const SUBCOMMANDS: [&str; 3] = ["extract", "validate", "pack-strip"];

#[test]
fn every_flag_the_cli_advertises_appears_in_the_readme() {
    let section = readme_soltool_section();
    let mut missing = Vec::new();

    for subcommand in SUBCOMMANDS {
        for flag in documented_flags(&[subcommand]) {
            // `-o` is spelled as the short form in the usage lines; the long
            // form is the one a reader would look up, so either counts.
            let long = section.contains(&flag);
            let short_output = flag == "--output" && section.contains("-o ");
            if !long && !short_output {
                missing.push(format!("{subcommand} {flag}"));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "flags the CLI advertises but the README does not mention: {missing:?}"
    );
}

/// Every subcommand is listed, so a new one cannot ship undocumented.
#[test]
fn every_subcommand_appears_in_the_readme() {
    let section = readme_soltool_section();
    for subcommand in SUBCOMMANDS {
        assert!(
            section.contains(subcommand),
            "the README does not mention `{subcommand}`"
        );
    }

    let top = documented_flags(&[]);
    assert!(
        top.is_empty(),
        "soltool grew a top-level flag this test does not check: {top:?}"
    );
}
