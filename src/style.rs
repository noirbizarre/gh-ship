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
    /// See [`decide`] for the precedence, which this only feeds.
    pub fn auto() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        Self {
            colored: decide(
                var("NO_COLOR").as_deref(),
                var("CLICOLOR").as_deref(),
                var("CLICOLOR_FORCE").as_deref(),
                var("TERM").as_deref(),
                var("GITHUB_ACTIONS").as_deref(),
                std::io::stderr().is_terminal(),
            ),
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
}

/// Decide whether to emit ANSI escapes.
///
/// Pure, and takes its inputs rather than reading them, so every rule below can
/// be tested exhaustively. Environment variables are process-global and Rust
/// runs tests in parallel, so a test that sets one races every other test in the
/// binary — this keeps that hazard out of the suite entirely.
///
/// Precedence, highest first:
///
/// | # | Condition | Result |
/// |---|---|---|
/// | 1 | `NO_COLOR` set and non-empty | off |
/// | 2 | `CLICOLOR=0` | off |
/// | 3 | `CLICOLOR_FORCE` set and not `0` | on |
/// | 4 | `TERM=dumb` | off |
/// | 5 | `GITHUB_ACTIONS=true` | on |
/// | 6 | stderr is a terminal | on |
/// | 7 | otherwise | off |
///
/// `NO_COLOR` leads because <https://no-color.org> requires it to.
///
/// `CLICOLOR_FORCE` deliberately outranks `TERM=dumb`: overriding a terminal
/// that claims it cannot render escapes is the entire purpose of a force switch.
///
/// Rule 5 exists because GitHub Actions renders ANSI in its logs while giving
/// every process a pipe rather than a terminal, so rule 6 alone would leave
/// every workflow monochrome. `GITHUB_ACTIONS` specifically, not a generic `CI`:
/// plenty of CI systems capture output that no one will ever render.
fn decide(
    no_color: Option<&str>,
    clicolor: Option<&str>,
    clicolor_force: Option<&str>,
    term: Option<&str>,
    github_actions: Option<&str>,
    is_tty: bool,
) -> bool {
    // An empty value means "not set" — that is how an unset variable usually
    // reaches a child process through a shell.
    fn set(v: Option<&str>) -> Option<&str> {
        v.filter(|s| !s.is_empty())
    }

    if set(no_color).is_some() {
        return false;
    }
    if set(clicolor) == Some("0") {
        return false;
    }
    if let Some(force) = set(clicolor_force)
        && force != "0"
    {
        return true;
    }
    if term == Some("dumb") {
        return false;
    }
    if set(github_actions) == Some("true") {
        return true;
    }
    is_tty
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
        assert_eq!(t.subject("release/next"), "release/next");
        assert_eq!(t.url("https://example.com"), "https://example.com");
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

    // --- colour precedence -------------------------------------------------
    //
    // `decide` takes its inputs rather than reading the environment, so these
    // exercise every rule without a single `set_var` — which would race the
    // rest of the suite, since environment variables are process-global and
    // tests run in parallel.

    /// Argument order is easy to get wrong with six positional parameters.
    #[derive(Default)]
    struct Env<'a> {
        no_color: Option<&'a str>,
        clicolor: Option<&'a str>,
        clicolor_force: Option<&'a str>,
        term: Option<&'a str>,
        github_actions: Option<&'a str>,
        tty: bool,
    }

    fn colour(env: Env) -> bool {
        decide(
            env.no_color,
            env.clicolor,
            env.clicolor_force,
            env.term,
            env.github_actions,
            env.tty,
        )
    }

    /// The reported bug: Actions gives every process a pipe, so TTY detection
    /// alone left every workflow log monochrome.
    #[test]
    fn github_actions_gets_colour_without_a_tty() {
        assert!(colour(Env {
            github_actions: Some("true"),
            ..Default::default()
        }));
    }

    #[test]
    fn only_github_actions_set_to_true_counts() {
        for value in ["false", "", "1", "yes"] {
            assert!(
                !colour(Env {
                    github_actions: Some(value),
                    ..Default::default()
                }),
                "GITHUB_ACTIONS={value:?} should not imply a rendering terminal"
            );
        }
    }

    /// <https://no-color.org> requires this to win over everything.
    #[test]
    fn no_color_beats_every_other_signal() {
        assert!(!colour(Env {
            no_color: Some("1"),
            clicolor_force: Some("1"),
            github_actions: Some("true"),
            tty: true,
            ..Default::default()
        }));
    }

    #[test]
    fn an_empty_no_color_is_not_set() {
        // Unset variables commonly reach a child process as empty strings.
        assert!(colour(Env {
            no_color: Some(""),
            tty: true,
            ..Default::default()
        }));
    }

    #[test]
    fn clicolor_force_enables_colour_without_a_tty() {
        assert!(colour(Env {
            clicolor_force: Some("1"),
            ..Default::default()
        }));
    }

    /// Overriding a terminal that claims it cannot render escapes is the whole
    /// point of a force switch.
    #[test]
    fn clicolor_force_overrides_a_dumb_terminal() {
        assert!(colour(Env {
            clicolor_force: Some("1"),
            term: Some("dumb"),
            ..Default::default()
        }));
    }

    #[test]
    fn clicolor_force_zero_does_not_force() {
        assert!(!colour(Env {
            clicolor_force: Some("0"),
            ..Default::default()
        }));
    }

    #[test]
    fn clicolor_zero_disables_colour_even_on_a_terminal() {
        assert!(!colour(Env {
            clicolor: Some("0"),
            tty: true,
            ..Default::default()
        }));
    }

    #[test]
    fn a_dumb_terminal_gets_no_colour() {
        assert!(!colour(Env {
            term: Some("dumb"),
            tty: true,
            ..Default::default()
        }));
        assert!(!colour(Env {
            term: Some("dumb"),
            github_actions: Some("true"),
            ..Default::default()
        }));
    }

    #[test]
    fn falls_back_to_terminal_detection() {
        assert!(colour(Env {
            tty: true,
            ..Default::default()
        }));
        assert!(!colour(Env::default()), "a pipe with no hints stays plain");
    }

    #[test]
    fn a_normal_terminal_is_unaffected_by_term() {
        assert!(colour(Env {
            term: Some("xterm-256color"),
            tty: true,
            ..Default::default()
        }));
    }

    /// The integration harness sets `NO_COLOR=1`, and CI sets
    /// `GITHUB_ACTIONS=true`. Without rule 1 outranking rule 5, every snapshot
    /// would sprout escape codes the moment the suite ran in CI.
    #[test]
    fn snapshots_stay_plain_when_the_suite_runs_in_actions() {
        assert!(!colour(Env {
            no_color: Some("1"),
            term: Some("dumb"),
            github_actions: Some("true"),
            ..Default::default()
        }));
    }
}
