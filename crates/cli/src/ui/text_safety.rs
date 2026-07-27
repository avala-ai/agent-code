//! Make invisible and direction-changing characters visible before text
//! reaches the terminal.
//!
//! A terminal reorders text according to embedded Unicode bidi controls,
//! so a string can *display* as something quite different from the bytes
//! it contains — the Trojan Source class of attack (CVE-2021-42574). For
//! an agent this is not academic: the approval modal is where the user
//! decides whether a command may run, and the transcript is where they
//! read file contents and diffs. If the rendering disagrees with the
//! bytes, the user authorizes something they did not read.
//!
//! ```text
//! bytes:     rm -rf /tmp/safe \u{202e}# hctap ylppa\u{202c}
//! displayed: rm -rf /tmp/safe # apply patch
//! ```
//!
//! The controls are **escaped, not stripped**, for two reasons: removing
//! them would silently change the text the user is reading, and escaping
//! keeps ordinary right-to-left writing working. Arabic and Hebrew
//! letters carry their own direction — only the explicit override and
//! isolate *controls* are escaped, so genuine RTL prose renders normally.
//!
//! This mirrors what rustc's `text_direction_codepoint_in_literal` lint
//! does for source code.

use std::borrow::Cow;

/// True for characters that change bidirectional rendering or occupy no
/// visible width, i.e. those that let displayed text diverge from the
/// underlying bytes.
fn is_deceptive(c: char) -> bool {
    matches!(c as u32,
        // Bidi embedding / override / pop: LRE RLE PDF LRO RLO
        0x202A..=0x202E
        // Bidi isolates: LRI RLI FSI PDI
        | 0x2066..=0x2069
        // Directional marks: LRM RLM ALM
        | 0x200E | 0x200F | 0x061C
        // Zero-width: ZWSP ZWNJ ZWJ, word joiner, BOM/ZWNBSP
        | 0x200B..=0x200D | 0x2060 | 0xFEFF
    )
}

/// Replace deceptive characters with a visible `<U+XXXX>` marker.
///
/// Returns the input untouched (and unallocated) when it contains none,
/// which is the overwhelmingly common case.
pub fn escape_deceptive(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_deceptive) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if is_deceptive(c) {
            out.push_str(&format!("<U+{:04X}>", c as u32));
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_ordinary_text_untouched_without_allocating() {
        for s in ["", "ls -la", "héllo wörld", "日本語のテキスト", "a\tb\nc"] {
            assert!(
                matches!(escape_deceptive(s), Cow::Borrowed(_)),
                "allocated for clean input: {s:?}"
            );
            assert_eq!(escape_deceptive(s), s);
        }
    }

    #[test]
    fn the_trojan_source_command_becomes_readable() {
        // Displays as `rm -rf /tmp/safe # apply patch` in a terminal that
        // honours the override — the user approves a comment and runs a
        // deletion.
        let attack = "rm -rf /tmp/safe \u{202e}# hctap ylppa\u{202c}";
        let safe = escape_deceptive(attack);
        assert_eq!(safe, "rm -rf /tmp/safe <U+202E># hctap ylppa<U+202C>");
        assert!(!safe.contains('\u{202e}'), "override survived: {safe}");
    }

    #[test]
    fn every_bidi_and_zero_width_control_is_escaped() {
        for c in [
            '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', // embed/override
            '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}', // isolates
            '\u{200E}', '\u{200F}', '\u{061C}', // marks
            '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', // zero-width
        ] {
            let input = c.to_string();
            let out = escape_deceptive(&input);
            assert_eq!(
                out,
                format!("<U+{:04X}>", c as u32),
                "missed {:04X}",
                c as u32
            );
        }
    }

    /// Escaping the controls must not disturb ordinary right-to-left
    /// writing: Arabic and Hebrew letters carry their own direction and
    /// are not controls.
    #[test]
    fn legitimate_rtl_prose_is_preserved() {
        for s in ["مرحبا بالعالم", "שלום עולם", "قالب: git commit"] {
            assert_eq!(escape_deceptive(s), s, "mangled RTL prose: {s}");
        }
    }

    #[test]
    fn a_zero_width_space_hiding_a_flag_is_revealed() {
        // `rm -rf /` with a zero-width space wedged in reads as clean.
        let hidden = "rm -\u{200b}rf /tmp/x";
        assert_eq!(escape_deceptive(hidden), "rm -<U+200B>rf /tmp/x");
    }
}
