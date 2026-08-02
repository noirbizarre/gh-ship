//! Lightweight similarity / span helpers used by validation diagnostics.
//!
//! `suggest` returns the best "did you mean?" candidate using
//! Jaro–Winkler with a 0.8 floor — close enough to catch typos like
//! `prepare-relase` → `prepare-release` without flagging completely unrelated
//! names.
//!
//! `span_of_substring` derives a [`miette::SourceSpan`] for a substring inside a
//! YAML source. Since `serde_norway::Value` does not retain positions,
//! we fall back to a naïve substring search: it is good enough for
//! the small, hand-written configs `gh ship` targets, and returns a
//! zero-length span at offset `0` when the needle is missing (miette
//! renders that without a label).

use miette::SourceSpan;
use strsim::jaro_winkler;

/// Similarity threshold above which a candidate is considered a likely
/// typo of the input. Tuned to accept `prepare-relase` → `prepare-release` while
/// rejecting unrelated names.
pub const SUGGEST_THRESHOLD: f64 = 0.8;

/// Return the single most-similar candidate above
/// [`SUGGEST_THRESHOLD`], or `None` when no candidate is close enough.
pub fn suggest<S: AsRef<str>>(input: &str, candidates: &[S]) -> Option<String> {
    candidates
        .iter()
        .map(|c| {
            let s = c.as_ref();
            (s, jaro_winkler(input, s))
        })
        .filter(|(_, score)| *score >= SUGGEST_THRESHOLD)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(s, _)| s.to_string())
}

/// Build a "did you mean `…`?" hint, or `None` when there's no good
/// candidate.
pub fn did_you_mean<S: AsRef<str>>(input: &str, candidates: &[S]) -> Option<String> {
    suggest(input, candidates).map(|s| format!("did you mean `{s}`?"))
}

/// Span the first occurrence of `needle` in `haystack`.
///
/// Falls back to a zero-length span at offset `0` when the needle is
/// not present — miette renders such spans without a caret, which is
/// what we want for diagnostics whose span we cannot recover.
pub fn span_of_substring(haystack: &str, needle: &str) -> SourceSpan {
    if needle.is_empty() {
        return SourceSpan::from((0usize, 0usize));
    }
    match haystack.find(needle) {
        Some(offset) => SourceSpan::from((offset, needle.len())),
        None => SourceSpan::from((0usize, 0usize)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_typos() {
        let cands = vec!["prepare-release", "publish-release", "docs"];
        assert_eq!(
            suggest("prepare-relase", &cands),
            Some("prepare-release".into())
        );
    }

    #[test]
    fn returns_none_for_unrelated_input() {
        let cands = vec!["prepare-release", "publish-release"];
        assert_eq!(suggest("totally_different", &cands), None);
    }

    #[test]
    fn did_you_mean_formats_hint() {
        let cands = vec!["release", "docs"];
        assert_eq!(
            did_you_mean("releas", &cands),
            Some("did you mean `release`?".into())
        );
    }

    #[test]
    fn span_of_substring_finds_it() {
        let span = span_of_substring("workflows:\n  prepare: prepare-relase\n", "prepare-relase");
        let offset: usize = span.offset();
        let len = span.len();
        assert_eq!(len, "prepare-relase".len());
        assert!(offset > 0);
    }

    #[test]
    fn span_of_substring_missing_is_empty() {
        let span = span_of_substring("hello", "world");
        assert_eq!(span.offset(), 0);
        assert_eq!(span.len(), 0);
    }
}
