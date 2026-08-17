//! End-to-end smoke test: boots the real binary in `--smoke` mode,
//! which builds the full chrome — window, real menu bar, status bar,
//! canvas, and both custom dialogs — renders frames through the
//! on-window wgpu path, exercises the dialog populate/read-back and
//! theme-switch paths, and exits. Any startup, chrome, or render
//! failure exits non-zero and fails this test.
//!
//! A second test below boots with a `--theme` override pointing at a
//! constructed fixture — a literal copy of the bundled default theme
//! under another id — so `--smoke`'s theme-switch step becomes a
//! genuine cross-theme switch whose display list nonetheless coincides
//! with the one already on screen. This environment otherwise has only
//! one discoverable theme, so building that fixture is the only way to
//! reach that branch at all.
//!
//! Lives in `tests/` (not the binary's unit tests) because only
//! integration tests make cargo build the real binary and expose its
//! path — `cargo test` compiles a bin crate's unit tests as their own
//! harness without producing the plain executable.
//!
//! Windows-only by nature (the chrome is); the Windows CI job runs it
//! on every push.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn smoke_boots_chrome_and_renders() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sol-win32"))
        .args(["--smoke", "--seed", "1"])
        .output()
        .expect("running sol-win32 --smoke");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "smoke run failed ({:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    // Rust-side failures (startup, render, autosave) log "sol-win32:".
    let failure = stderr.lines().find(|line| line.contains("sol-win32:"));
    assert_eq!(failure, None, "frontend errors during smoke run:\n{stderr}");
}

/// Recursively copies every file under `src` into `dst` (subdirectories
/// created as needed) — builds a second theme fixture whose manifest,
/// and therefore whose card geometry, is byte-for-byte identical to the
/// bundled default theme's, just reachable under a different id (the one
/// `dst`'s directory name gives it).
fn copy_theme_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("creating a fixture theme directory");
    for entry in std::fs::read_dir(src).expect("reading a source theme directory") {
        let entry = entry.expect("reading a theme directory entry");
        let target = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_theme_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copying a theme asset");
        }
    }
}

/// Proves `switch_theme`'s forced render (`push_frame`, in `ui.rs`)
/// actually matters, the same way the scaling test proves it for
/// `switch_scaling`: builds a second theme — a literal copy of the
/// bundled default under another id — and boots with it active via
/// `--theme`. `smoke()`'s existing theme-switch step then switches from
/// it back to `"default"`: a genuine cross-theme switch (`adopt_theme`
/// returns `true`) whose display list nonetheless coincides byte-for-byte
/// with the one already on screen, since both themes share every
/// manifest field that affects card geometry. Only the forced push can
/// put the freshly rebuilt atlas on screen in that case; the per-tick
/// change-gated path cannot tell the two themes apart. `smoke()` prints
/// a `"cross-theme switch"` line only when this branch actually ran and
/// a fresh frame landed, which this test asserts on.
#[test]
fn smoke_switches_between_themes_with_a_coincident_display_list() {
    let default_dir = sol_frontend::themes::dev_default_dir()
        .expect("the in-tree default theme must be discoverable to build this fixture");
    let fixture_root = tempfile::tempdir().expect("creating a fixture tempdir");
    let fixture_theme = fixture_root.path().join("coincident");
    copy_theme_tree(&default_dir, &fixture_theme);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sol-win32"))
        .args(["--smoke", "--seed", "1", "--theme"])
        .arg(&fixture_theme)
        .output()
        .expect("running sol-win32 --smoke --theme <fixture>");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "smoke run failed ({:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    let failure = stderr.lines().find(|line| line.contains("sol-win32:"));
    assert_eq!(failure, None, "frontend errors during smoke run:\n{stderr}");
    assert!(
        stdout.contains("cross-theme switch"),
        "expected the cross-theme switch to actually run with a second theme \
         present:\n{stdout}"
    );
}
