//! Byte-span lookup for JSON pointers.
//!
//! `boon` reports failures by JSON pointer (`/release/notes`), but a good
//! diagnostic needs a byte range in the *original* source so miette can
//! draw a caret under the offending token. `serde_json` discards
//! positions, so we re-scan the raw text.
//!
//! This is a position-only scanner: it never materialises values, it
//! only tracks where they start and end. It assumes the input already
//! parsed successfully as JSON (we always parse before validating), so
//! malformed input simply yields `None` rather than a parse error.

use miette::SourceSpan;

/// One step in a JSON pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// An object property.
    Prop(String),
    /// An array index.
    Index(usize),
}

/// Parse a JSON Pointer (RFC 6901) into steps.
///
/// Numeric steps stay ambiguous (`/items/0` could be a property named
/// `0`), so we emit `Prop` and let [`locate`] try the index
/// interpretation when it encounters an array.
pub fn parse_pointer(pointer: &str) -> Vec<Step> {
    pointer
        .split('/')
        .skip(1) // leading empty segment before the first `/`
        .filter(|s| !s.is_empty() || pointer == "/")
        .map(|s| Step::Prop(s.replace("~1", "/").replace("~0", "~")))
        .collect()
}

/// Locate the value at `path` and return its span in `src`.
///
/// Returns the span of the whole document when `path` is empty, and
/// `None` when the path does not resolve.
pub fn locate(src: &str, path: &[Step]) -> Option<SourceSpan> {
    let mut sc = Scanner {
        b: src.as_bytes(),
        pos: 0,
    };
    sc.ws();
    sc.value(path).map(|(s, e)| SourceSpan::from(s..e))
}

/// Locate the *key* token of an object property, rather than its value.
///
/// Used for `additionalProperties` diagnostics, where the useful caret
/// sits under the unexpected key itself.
pub fn locate_key(src: &str, parent: &[Step], key: &str) -> Option<SourceSpan> {
    let mut sc = Scanner {
        b: src.as_bytes(),
        pos: 0,
    };
    sc.ws();
    sc.key(parent, key).map(|(s, e)| SourceSpan::from(s..e))
}

struct Scanner<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn ws(&mut self) {
        while self.pos < self.b.len() && self.b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    /// Consume a string literal, returning its span *including* quotes,
    /// plus the decoded-enough content for comparison.
    fn string(&mut self) -> Option<(usize, usize, String)> {
        let start = self.pos;
        if self.bump()? != b'"' {
            return None;
        }
        let mut out = String::new();
        loop {
            match self.bump()? {
                b'"' => return Some((start, self.pos, out)),
                b'\\' => match self.bump()? {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'/' => out.push('/'),
                    b'\\' => out.push('\\'),
                    b'"' => out.push('"'),
                    b'u' => {
                        // We only need enough fidelity to compare keys;
                        // surrogate pairs in property names are
                        // vanishingly rare, so decode the BMP case and
                        // fall back to a replacement char otherwise.
                        let hex = self.b.get(self.pos..self.pos + 4)?;
                        self.pos += 4;
                        let n = u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                        out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
                    }
                    _ => return None,
                },
                c => {
                    // Multi-byte UTF-8 sequences pass through verbatim;
                    // we re-assemble them from the raw bytes.
                    let len = utf8_len(c);
                    if len == 1 {
                        out.push(c as char);
                    } else {
                        let end = self.pos - 1 + len;
                        let s = std::str::from_utf8(self.b.get(self.pos - 1..end)?).ok()?;
                        out.push_str(s);
                        self.pos = end;
                    }
                }
            }
        }
    }

    /// Scan a value. When `path` is empty, returns the span of that
    /// value; otherwise descends according to `path`.
    fn value(&mut self, path: &[Step]) -> Option<(usize, usize)> {
        self.ws();
        let start = self.pos;
        match self.peek()? {
            b'{' => {
                let target = path.first();
                self.pos += 1;
                loop {
                    self.ws();
                    match self.peek()? {
                        b'}' => {
                            self.pos += 1;
                            break;
                        }
                        b',' => {
                            self.pos += 1;
                            continue;
                        }
                        b'"' => {}
                        _ => return None,
                    }
                    let (_, _, key) = self.string()?;
                    self.ws();
                    if self.bump()? != b':' {
                        return None;
                    }
                    let matches = matches!(target, Some(Step::Prop(p)) if *p == key);
                    if matches {
                        return self.value(&path[1..]);
                    }
                    self.value(&[])?;
                }
                if path.is_empty() {
                    Some((start, self.pos))
                } else {
                    None
                }
            }
            b'[' => {
                let target = index_of(path.first());
                self.pos += 1;
                let mut i = 0usize;
                loop {
                    self.ws();
                    match self.peek()? {
                        b']' => {
                            self.pos += 1;
                            break;
                        }
                        b',' => {
                            self.pos += 1;
                            continue;
                        }
                        _ => {}
                    }
                    if target == Some(i) {
                        return self.value(&path[1..]);
                    }
                    self.value(&[])?;
                    i += 1;
                }
                if path.is_empty() {
                    Some((start, self.pos))
                } else {
                    None
                }
            }
            b'"' => {
                let (s, e, _) = self.string()?;
                path.is_empty().then_some((s, e))
            }
            _ => {
                // Scalar: number, true, false, null.
                while let Some(c) = self.peek() {
                    if c.is_ascii_whitespace() || c == b',' || c == b'}' || c == b']' {
                        break;
                    }
                    self.pos += 1;
                }
                if self.pos == start {
                    return None;
                }
                path.is_empty().then_some((start, self.pos))
            }
        }
    }

    /// Find the span of the key token `key` inside the object at `parent`.
    fn key(&mut self, parent: &[Step], key: &str) -> Option<(usize, usize)> {
        self.ws();
        if !parent.is_empty() {
            // Descend to the parent object first, then re-scan from there.
            let (s, _) = self.value(parent)?;
            self.pos = s;
        }
        self.ws();
        if self.peek()? != b'{' {
            return None;
        }
        self.pos += 1;
        loop {
            self.ws();
            match self.peek()? {
                b'}' => return None,
                b',' => {
                    self.pos += 1;
                    continue;
                }
                b'"' => {}
                _ => return None,
            }
            let (ks, ke, k) = self.string()?;
            self.ws();
            if self.bump()? != b':' {
                return None;
            }
            if k == key {
                return Some((ks, ke));
            }
            self.value(&[])?;
        }
    }
}

fn index_of(step: Option<&Step>) -> Option<usize> {
    match step {
        Some(Step::Index(i)) => Some(*i),
        Some(Step::Prop(p)) => p.parse().ok(),
        None => None,
    }
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(src: &str, span: SourceSpan) -> &str {
        &src[span.offset()..span.offset() + span.len()]
    }

    #[test]
    fn parses_pointers() {
        assert_eq!(parse_pointer(""), vec![]);
        assert_eq!(parse_pointer("/tag"), vec![Step::Prop("tag".into())]);
        assert_eq!(
            parse_pointer("/release/notes"),
            vec![Step::Prop("release".into()), Step::Prop("notes".into())]
        );
        assert_eq!(
            parse_pointer("/a~1b"),
            vec![Step::Prop("a/b".into())],
            "~1 decodes to /"
        );
    }

    #[test]
    fn locates_top_level_scalar() {
        let src = r#"{"schemaVersion": 1, "changed": true}"#;
        assert_eq!(
            text(src, locate(src, &parse_pointer("/changed")).unwrap()),
            "true"
        );
        assert_eq!(
            text(src, locate(src, &parse_pointer("/schemaVersion")).unwrap()),
            "1"
        );
    }

    #[test]
    fn locates_nested_string_with_quotes() {
        let src = r#"{"release": {"name": "Big One", "notes": "a\nb"}}"#;
        assert_eq!(
            text(src, locate(src, &parse_pointer("/release/name")).unwrap()),
            r#""Big One""#
        );
        assert_eq!(
            text(src, locate(src, &parse_pointer("/release/notes")).unwrap()),
            r#""a\nb""#,
            "escapes must not shift the span"
        );
    }

    #[test]
    fn locates_whole_object() {
        let src = r#"{"release": {"name": "x"}}"#;
        assert_eq!(
            text(src, locate(src, &parse_pointer("/release")).unwrap()),
            r#"{"name": "x"}"#
        );
        assert_eq!(text(src, locate(src, &[]).unwrap()), src);
    }

    #[test]
    fn locates_array_items() {
        let src = r#"{"pull_request": {"labels": ["a", "bb", "ccc"]}}"#;
        let p = parse_pointer("/pull_request/labels/1");
        assert_eq!(text(src, locate(src, &p).unwrap()), r#""bb""#);
    }

    #[test]
    fn locates_key_token() {
        let src = "{\n  \"changed\": true,\n  \"bogus\": 1\n}";
        assert_eq!(
            text(src, locate_key(src, &[], "bogus").unwrap()),
            r#""bogus""#
        );
    }

    #[test]
    fn locates_key_in_nested_object() {
        let src = r#"{"release": {"name": "x", "nope": 1}}"#;
        let span = locate_key(src, &parse_pointer("/release"), "nope").unwrap();
        assert_eq!(text(src, span), r#""nope""#);
    }

    #[test]
    fn missing_paths_return_none() {
        let src = r#"{"a": 1}"#;
        assert!(locate(src, &parse_pointer("/b")).is_none());
        assert!(locate(src, &parse_pointer("/a/b")).is_none());
        assert!(locate_key(src, &[], "zzz").is_none());
    }

    #[test]
    fn handles_multiline_and_unicode() {
        let src = "{\n  \"tag\": \"v1.0.0\",\n  \"emoji\": \"🚀 ship\"\n}";
        assert_eq!(
            text(src, locate(src, &parse_pointer("/tag")).unwrap()),
            "\"v1.0.0\""
        );
        assert_eq!(
            text(src, locate(src, &parse_pointer("/emoji")).unwrap()),
            "\"🚀 ship\""
        );
    }
}
