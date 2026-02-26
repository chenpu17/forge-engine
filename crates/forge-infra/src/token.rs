//! Unified token estimation utilities.
//!
//! Keeps estimation local/fast (no network calls) while being more robust
//! for code-heavy and multilingual text than a plain `chars / 4` heuristic.

#[derive(Default, Debug, Clone, Copy)]
struct CharProfile {
    ascii_alnum: usize,
    ascii_punct: usize,
    whitespace: usize,
    cjk: usize,
    emoji: usize,
    other_non_ascii: usize,
}

#[inline]
const fn div_ceil(n: usize, d: usize) -> usize {
    n.div_ceil(d)
}

#[inline]
const fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0x3040..=0x309F
            | 0x30A0..=0x30FF
            | 0x31F0..=0x31FF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
    )
}

#[inline]
const fn is_emoji(c: char) -> bool {
    matches!(
        c as u32,
        0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0xFE00..=0xFE0F
    )
}

fn profile_text(text: &str) -> CharProfile {
    let mut profile = CharProfile::default();
    for ch in text.chars() {
        if ch.is_whitespace() {
            profile.whitespace += 1;
        } else if ch.is_ascii_alphanumeric() {
            profile.ascii_alnum += 1;
        } else if ch.is_ascii() {
            profile.ascii_punct += 1;
        } else if is_cjk(ch) {
            profile.cjk += 1;
        } else if is_emoji(ch) {
            profile.emoji += 1;
        } else {
            profile.other_non_ascii += 1;
        }
    }
    profile
}

/// Estimate token count for text content.
///
/// Heuristic model:
/// - ASCII words/identifiers: ~4 chars/token
/// - ASCII punctuation/symbols: denser splitting
/// - CJK scripts: ~2 chars/token
/// - Emoji and other non-ASCII get dedicated buckets
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    let profile = profile_text(text);
    let estimate = div_ceil(profile.ascii_alnum, 4)
        + div_ceil(profile.ascii_punct, 2)
        + (profile.whitespace / 8)
        + div_ceil(profile.cjk, 2)
        + (profile.emoji * 2)
        + div_ceil(profile.other_non_ascii, 3);
    estimate.max(1)
}

/// Estimate token count using a character-length ratio.
///
/// Uses Unicode character count (not byte length) for correct CJK handling.
#[must_use]
pub fn estimate_tokens_by_ratio(text: &str, chars_per_token: f64) -> usize {
    if text.is_empty() {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let result = (text.chars().count() as f64 / chars_per_token).ceil() as usize;
    result
}

/// Fast lower-bound estimate for truncation decisions.
///
/// Intentionally underestimates relative to [`estimate_tokens`].
#[must_use]
pub fn estimate_tokens_fast(text: &str) -> usize {
    let profile = profile_text(text);
    (profile.ascii_alnum / 4)
        + (profile.ascii_punct / 3)
        + (profile.whitespace / 12)
        + (profile.cjk / 2)
        + profile.emoji
        + (profile.other_non_ascii / 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_ascii() {
        assert_eq!(estimate_tokens("hello world"), 3);
    }

    #[test]
    fn test_estimate_tokens_cjk() {
        assert_eq!(estimate_tokens("你好世界"), 2);
    }

    #[test]
    fn test_estimate_tokens_mixed() {
        assert_eq!(estimate_tokens("hello 你好"), 3);
    }

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 1);
    }

    #[test]
    fn test_estimate_tokens_by_ratio() {
        let text = "a".repeat(100);
        assert_eq!(estimate_tokens_by_ratio(&text, 4.0), 25);
        assert_eq!(estimate_tokens_by_ratio("", 4.0), 0);
    }

    #[test]
    fn test_estimate_tokens_fast() {
        assert_eq!(estimate_tokens_fast("a".repeat(100).as_str()), 25);
        assert_eq!(estimate_tokens_fast(""), 0);
    }

    #[test]
    fn test_estimate_tokens_emoji() {
        let result = estimate_tokens("🎉🎊🎈");
        assert!(result >= 1, "emoji should produce at least 1 token");
    }

    #[test]
    fn test_estimate_tokens_fast_cjk() {
        let result = estimate_tokens_fast("你好世界测试");
        assert!(result >= 1, "CJK fast estimate should be >= 1");
    }
}
