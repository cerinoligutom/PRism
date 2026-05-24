//! Word-boundary aware `@<viewer>` matcher used by the per-PR mention scan.

/// Count `@<viewer>` matches in `body`, treating a match as terminated by
/// whitespace, EOL, ASCII punctuation, or end-of-string. Case-insensitive
/// because GitHub logins normalise that way. Rejects subword extensions like
/// `@<viewer>-bot` or `@<viewer>123`. See ADR 0015 and the M4 contract for
/// the word-boundary spec.
///
/// Returns `true` if at least one match is found. Callers count comment rows
/// that match (so two mentions in the same comment count as one increment),
/// matching the contract's row-count semantics in `docs/contracts/triage-ux.md`.
pub(super) fn mentions_viewer(body: &str, viewer_login: &str) -> bool {
    if viewer_login.is_empty() || body.is_empty() {
        return false;
    }
    let needle = viewer_login.to_lowercase();
    let body_lower = body.to_lowercase();
    let needle_bytes = needle.as_bytes();
    let body_bytes = body_lower.as_bytes();
    let nlen = needle_bytes.len();
    let blen = body_bytes.len();

    let mut cursor = 0;
    while cursor < blen {
        let Some(at_offset) = body_bytes[cursor..].iter().position(|&b| b == b'@') else {
            return false;
        };
        let login_start = cursor + at_offset + 1;
        let login_end = login_start + nlen;
        if login_end <= blen && &body_bytes[login_start..login_end] == needle_bytes {
            let trailing = body_bytes.get(login_end).copied();
            if is_mention_boundary(trailing) {
                return true;
            }
        }
        // Advance past this `@` regardless of match outcome to find the next.
        cursor = login_start;
    }
    false
}

/// Trailing-character predicate for the word-boundary spec. `None` means EOL.
/// Whitespace, common ASCII punctuation, and closing brackets all terminate a
/// mention; alphanumerics, hyphens, and underscores continue it (so
/// `@alice-bot` rejects when viewer is `alice`). Non-ASCII bytes fall through
/// as non-boundary to stay conservative against partial UTF-8 sequences.
fn is_mention_boundary(c: Option<u8>) -> bool {
    let Some(c) = c else {
        return true;
    };
    matches!(
        c,
        b' ' | b'\t'
            | b'\n'
            | b'\r'
            | b'.'
            | b','
            | b';'
            | b':'
            | b'!'
            | b'?'
            | b')'
            | b']'
            | b'}'
            | b'\''
            | b'"'
            | b'`'
            | b'/'
            | b'\\'
            | b'<'
            | b'>'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_viewer_matches_bare_login() {
        assert!(mentions_viewer("hey @alice please review", "alice"));
    }

    #[test]
    fn mentions_viewer_matches_at_end_of_string() {
        assert!(mentions_viewer("ping @alice", "alice"));
    }

    #[test]
    fn mentions_viewer_matches_with_trailing_punctuation() {
        for body in [
            "@alice,", "@alice.", "@alice!", "@alice?", "@alice:", "@alice;",
        ] {
            assert!(mentions_viewer(body, "alice"), "body {body:?} should match");
        }
    }

    #[test]
    fn mentions_viewer_rejects_subword_extension() {
        assert!(!mentions_viewer("ping @alice-bot for help", "alice"));
        assert!(!mentions_viewer("@alicia is here", "alice"));
        assert!(!mentions_viewer("@alice_two reviewed", "alice"));
        assert!(!mentions_viewer("@alice123", "alice"));
    }

    #[test]
    fn mentions_viewer_is_case_insensitive() {
        assert!(mentions_viewer("ping @ALICE today", "alice"));
        assert!(mentions_viewer("ping @alice today", "Alice"));
    }

    #[test]
    fn mentions_viewer_returns_false_on_empty_inputs() {
        assert!(!mentions_viewer("", "alice"));
        assert!(!mentions_viewer("hi @alice", ""));
    }

    #[test]
    fn mentions_viewer_skips_past_unrelated_at_signs() {
        assert!(mentions_viewer(
            "email me at user@example.com or @alice",
            "alice"
        ));
    }

    #[test]
    fn mentions_viewer_handles_at_near_end_without_login() {
        assert!(!mentions_viewer("trailing @", "alice"));
        assert!(!mentions_viewer("trailing @al", "alice"));
    }
}
