//! Output rendering.
//!
//! Every function here is pure: it takes a [`Theme`] and returns a
//! `String`. Nothing writes to a stream, nothing captures time, nothing
//! spins. That makes the entire user-facing output trivially snapshot
//! testable, and identical in a terminal, a CI log and a redirected
//! file.

use crate::style::Theme;

/// The product name as it appears in banners.
pub const TOOL: &str = "gh ship";

/// `▶ <action> <subject>`
pub fn action(theme: &Theme, verb: &str, subject: &str) -> String {
    format!("{} {verb} {}", theme.running("▶"), theme.subject(subject))
}

/// `✔ <message>`
pub fn ok(theme: &Theme, message: &str) -> String {
    format!("{} {message}", theme.success("✔"))
}

/// `✘ <message>`
pub fn fail(theme: &Theme, message: &str) -> String {
    format!("{} {message}", theme.failure("✘"))
}

/// `⚠ <message>`
pub fn warn(theme: &Theme, message: &str) -> String {
    format!("{} {message}", theme.warning("⚠"))
}

/// `- <message>`, dimmed — for no-ops and skips.
pub fn skip(theme: &Theme, message: &str) -> String {
    format!("{} {}", theme.skipped("-"), theme.skipped(message))
}

/// An indented `key: value` detail line.
pub fn detail(theme: &Theme, key: &str, value: &str) -> String {
    format!("  {} {value}", theme.key(&format!("{key}:")))
}

/// An indented detail line whose value is a URL.
pub fn detail_url(theme: &Theme, key: &str, url: &str) -> String {
    format!("  {} {}", theme.key(&format!("{key}:")), theme.url(url))
}

/// A horizontal rule used to frame rendered PR bodies.
pub fn rule(theme: &Theme, label: &str) -> String {
    let bar = "─".repeat(60usize.saturating_sub(label.chars().count() + 3));
    format!("{} {}", theme.dim(&format!("── {label}")), theme.dim(&bar))
}

/// The `nothing to release` outcome.
///
/// This is a success, not a warning: a workflow reporting `changed:
/// false` is the system working correctly.
pub fn nothing_to_release(theme: &Theme) -> String {
    format!(
        "{} {}",
        theme.success("✔"),
        "nothing to release — the prepare workflow reported no changes"
    )
}

/// Format a duration for human consumption.
///
/// Deliberately coarse: sub-millisecond precision is noise when the
/// thing being measured is a GitHub Actions run.
pub fn duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs > 0 {
        format!("{}.{:02}s", secs, d.subsec_millis() / 10)
    } else if d.as_millis() > 0 {
        format!("{}ms", d.as_millis())
    } else {
        "<1ms".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn plain_output_has_no_escapes() {
        let t = Theme::plain();
        for s in [
            action(&t, "dispatching", "prepare-release"),
            ok(&t, "done"),
            fail(&t, "nope"),
            warn(&t, "careful"),
            skip(&t, "skipped"),
            detail(&t, "branch", "release/next"),
            detail_url(&t, "pr", "https://example.com/1"),
            rule(&t, "Pull Request"),
            nothing_to_release(&t),
        ] {
            assert!(!s.contains('\x1b'), "unexpected ANSI in plain mode: {s:?}");
        }
    }

    #[test]
    fn markers_are_stable() {
        let t = Theme::plain();
        assert_eq!(action(&t, "dispatching", "wf"), "▶ dispatching wf");
        assert_eq!(ok(&t, "done"), "✔ done");
        assert_eq!(fail(&t, "nope"), "✘ nope");
        assert_eq!(warn(&t, "hmm"), "⚠ hmm");
        assert_eq!(skip(&t, "meh"), "- meh");
        assert_eq!(detail(&t, "branch", "main"), "  branch: main");
    }

    #[test]
    fn durations_are_coarse_and_readable() {
        assert_eq!(duration(Duration::from_micros(10)), "<1ms");
        assert_eq!(duration(Duration::from_millis(250)), "250ms");
        assert_eq!(duration(Duration::from_millis(1500)), "1.50s");
        assert_eq!(duration(Duration::from_secs(83)), "1m 23s");
    }

    #[test]
    fn rule_never_underflows() {
        let t = Theme::plain();
        let long = "x".repeat(200);
        let _ = rule(&t, &long); // must not panic
    }
}
