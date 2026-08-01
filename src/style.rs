//! Centralised terminal styling helpers.
//!
//! All colour decisions live behind a [`Theme`]. Callers obtain a theme
//! once (typically [`Theme::auto`] in `main`) and pass it to the logger;
//! tests use [`Theme::plain`] for deterministic, copy-paste-friendly
//! output.
//!
//! The module deliberately avoids spinners, animations, and any other
//! non-deterministic rendering: output is a single, append-only stream
//! that reads identically in a terminal, a CI log, and a redirected
//! file. This matters because `gh ship` spends most of its time waiting
//! on GitHub Actions, and those waits are logged, not animated.

use std::io::IsTerminal;

use owo_colors::{OwoColorize, Style};

/// Visual theme for log output.
///
/// A theme is just a flag plus a small palette: when `colored` is
/// `false`, every helper returns its input unchanged, so the same
/// formatting code paths are used in both modes.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    colored: bool,
}

impl Theme {
    /// A theme that emits plain ASCII only.
    ///
    /// This is what tests and `NO_COLOR` runs use. The output is
    /// byte-for-byte stable and safe to embed in golden files.
    pub const fn plain() -> Self {
        Self { colored: false }
    }

    /// A theme that always emits ANSI escapes.
    pub const fn colored() -> Self {
        Self { colored: true }
    }

    /// Pick a theme automatically from the environment.
    ///
    /// Returns [`Self::colored`] when stderr is a terminal and the
    /// `NO_COLOR` environment variable is unset (or empty); otherwise
    /// returns [`Self::plain`]. This honours the de-facto
    /// <https://no-color.org/> convention.
    pub fn auto() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let is_tty = std::io::stderr().is_terminal();
        Self {
            colored: !no_color && is_tty,
        }
    }

    /// Whether this theme emits ANSI escapes.
    pub fn is_colored(&self) -> bool {
        self.colored
    }

    fn paint(&self, s: &str, style: Style) -> String {
        if self.colored {
            s.style(style).to_string()
        } else {
            s.to_string()
        }
    }

    // --- Palette ---------------------------------------------------------

    /// Dimmed text — separators and ancillary detail.
    pub fn dim(&self, s: &str) -> String {
        self.paint(s, Style::new().dimmed())
    }

    /// The product name (`gh ship`) — cyan + bold to anchor banners.
    pub fn tool(&self, s: &str) -> String {
        self.paint(s, Style::new().cyan().bold())
    }

    /// A subject being acted on (workflow name, branch, tag) — bold.
    pub fn subject(&self, s: &str) -> String {
        self.paint(s, Style::new().bold())
    }

    /// Success marker (`✔`) — green + bold.
    pub fn success(&self, s: &str) -> String {
        self.paint(s, Style::new().green().bold())
    }

    /// Failure marker (`✘`) — red + bold.
    pub fn failure(&self, s: &str) -> String {
        self.paint(s, Style::new().red().bold())
    }

    /// In-progress marker (`▶`) — blue + bold.
    pub fn running(&self, s: &str) -> String {
        self.paint(s, Style::new().blue().bold())
    }

    /// Warning text (`⚠`) — yellow.
    pub fn warning(&self, s: &str) -> String {
        self.paint(s, Style::new().yellow())
    }

    /// Skipped / no-op — dimmed.
    pub fn skipped(&self, s: &str) -> String {
        self.paint(s, Style::new().dimmed())
    }

    /// A URL — underlined so it stands out as clickable.
    pub fn url(&self, s: &str) -> String {
        self.paint(s, Style::new().blue().underline())
    }

    /// A field key in a key/value listing — dimmed.
    pub fn key(&self, s: &str) -> String {
        self.paint(s, Style::new().dimmed())
    }

    /// A JSON pointer / config path in a diagnostic — magenta.
    pub fn pointer(&self, s: &str) -> String {
        self.paint(s, Style::new().magenta().bold())
    }

    /// Elapsed time — dimmed so it never competes with the marker it
    /// accompanies.
    pub fn duration(&self, s: &str) -> String {
        self.paint(s, Style::new().dimmed())
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::plain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_theme_returns_input_unchanged() {
        let t = Theme::plain();
        assert_eq!(t.dim("x"), "x");
        assert_eq!(t.success("ok"), "ok");
        assert_eq!(t.failure("nope"), "nope");
        assert_eq!(t.warning("careful"), "careful");
        assert_eq!(t.skipped("zzz"), "zzz");
        assert_eq!(t.tool("gh ship"), "gh ship");
        assert_eq!(t.subject("release/next"), "release/next");
        assert_eq!(t.url("https://example.com"), "https://example.com");
        assert_eq!(t.pointer("/release/notes"), "/release/notes");
        assert_eq!(t.duration("1.23s"), "1.23s");
        assert!(!t.is_colored());
    }

    #[test]
    fn colored_theme_wraps_with_ansi() {
        let t = Theme::colored();
        let s = t.success("ok");
        assert!(s.starts_with("\x1b["), "missing CSI: {s:?}");
        assert!(s.ends_with("\x1b[0m"), "missing reset: {s:?}");
        assert!(s.contains("ok"), "payload missing: {s:?}");
        assert!(t.is_colored());
    }

    #[test]
    fn colored_helpers_use_distinct_codes() {
        let t = Theme::colored();
        assert_ne!(t.success("x"), t.failure("x"));
        assert_ne!(t.success("x"), t.warning("x"));
        assert_ne!(t.running("x"), t.url("x"));
        assert_ne!(t.dim("x"), t.subject("x"));
    }

    #[test]
    fn auto_respects_no_color() {
        // `NO_COLOR` set to a non-empty value always disables colour,
        // regardless of TTY state. We cannot portably fake a TTY here,
        // so we assert the one direction that is deterministic.
        unsafe { std::env::set_var("NO_COLOR", "1") };
        assert!(!Theme::auto().is_colored());
        unsafe { std::env::remove_var("NO_COLOR") };
    }
}
