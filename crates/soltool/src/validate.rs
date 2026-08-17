//! `soltool validate <theme>`: the loader validates completeness and
//! returns typed errors, and `soltool validate` reuses that. This
//! module is intentionally a thin shell over
//! [`sol_theme::Theme::load_path`], adding only a one-line success summary.

use std::path::Path;

use sol_theme::{RenderMode, Theme, ThemeError};

/// Loads and validates the theme package at `theme` — a directory, or a
/// file tried as a zip archive, exactly [`sol_theme::Theme::load_path`]'s
/// semantics — returning a one-line summary naming the theme's name,
/// render mode, back count, and placeholder count on success.
///
/// ```
/// use std::path::Path;
///
/// use soltool::validate;
///
/// let error = validate::run(Path::new("/no/such/theme/at/all")).unwrap_err();
/// assert!(error.to_string().contains("/no/such/theme/at/all"));
/// ```
///
/// # Errors
///
/// Returns the [`ThemeError`] [`sol_theme::Theme::load_path`] itself
/// returns. Its `Display` already carries the full source chain — which
/// asset failed and why — so callers can print it to the user as-is.
pub fn run(theme: &Path) -> Result<String, ThemeError> {
    let theme = Theme::load_path(theme)?;
    let back_count = theme.backs().len();
    let placeholder_count = theme.placeholders().entries().count();
    Ok(format!(
        "{name}: valid ({mode} theme, {back_count} back{plural}, \
         {placeholder_count} placeholder{placeholder_plural})",
        name = theme.manifest.name,
        mode = render_mode_label(theme.manifest.render_mode),
        plural = if back_count == 1 { "" } else { "s" },
        placeholder_plural = if placeholder_count == 1 { "" } else { "s" },
    ))
}

/// The exact lowercase `render_mode` spelling, for the summary line.
fn render_mode_label(mode: RenderMode) -> &'static str {
    match mode {
        RenderMode::Png => "png",
        RenderMode::Vector => "vector",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_mode_labels_match_the_manifest_spellings() {
        assert_eq!(render_mode_label(RenderMode::Png), "png");
        assert_eq!(render_mode_label(RenderMode::Vector), "vector");
    }
}
