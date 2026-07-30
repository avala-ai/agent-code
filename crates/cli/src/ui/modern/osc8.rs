//! OSC 8 hyperlink emission (#558 D3-10 / D10-12).
//!
//! Ratatui's cell buffer has no hyperlink attribute, so after each frame we
//! re-stamp visible link runs with OSC 8 open/close around the already-drawn
//! label text. Capability-gated by [`super::terminal_caps::TerminalCaps::osc8_hyperlinks`].

use std::io::Write;

/// One hyperlink region to wrap with OSC 8 after the frame is painted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc8Span {
    pub x: u16,
    pub y: u16,
    pub url: String,
    pub label: String,
}

/// Build the CSI+OSC sequence for one span (1-based cursor addressing).
///
/// Returns `None` when the URL is rejected (non-http(s)/mailto or controls).
pub fn format_osc8_span(span: &Osc8Span) -> Option<String> {
    let url = sanitize_osc8_param(&span.url)?;
    if !is_allowed_scheme(&url) {
        return None;
    }
    let label = sanitize_osc8_label(&span.label);
    if label.is_empty() {
        return None;
    }
    // CSI row;col H  OSC 8 ;; url ST  label  OSC 8 ;; ST
    Some(format!(
        "\x1b[{};{}H\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
        span.y.saturating_add(1),
        span.x.saturating_add(1),
        url,
        label
    ))
}

/// Write every pending span to `out` and clear the queue. Best-effort.
pub fn emit_osc8_spans<W: Write>(out: &mut W, spans: &mut Vec<Osc8Span>) {
    if spans.is_empty() {
        return;
    }
    for span in spans.drain(..) {
        if let Some(seq) = format_osc8_span(&span) {
            let _ = out.write_all(seq.as_bytes());
        }
    }
    let _ = out.flush();
}

/// Strip C0/C1/DEL so a model-authored URL cannot break out of OSC 8 via
/// embedded BEL/ESC/ST. Returns `None` if nothing printable remains.
fn sanitize_osc8_param(s: &str) -> Option<String> {
    let cleaned: String = s
        .chars()
        .filter(|c| {
            let u = *c as u32;
            !matches!(u, 0x00..=0x1F | 0x7F | 0x80..=0x9F)
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn sanitize_osc8_label(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let u = *c as u32;
            !matches!(u, 0x00..=0x1F | 0x7F | 0x80..=0x9F)
        })
        .collect()
}

fn is_allowed_scheme(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_includes_url_and_label_with_1based_cursor() {
        let span = Osc8Span {
            x: 3,
            y: 5,
            url: "https://example.com/a".into(),
            label: "example".into(),
        };
        let seq = format_osc8_span(&span).expect("ok");
        assert!(seq.starts_with("\x1b[6;4H"), "got {seq:?}");
        assert!(seq.contains("\x1b]8;;https://example.com/a\x1b\\"));
        assert!(seq.contains("example"));
        assert!(seq.ends_with("\x1b]8;;\x1b\\"));
    }

    #[test]
    fn rejects_control_breakout_in_url() {
        let span = Osc8Span {
            x: 0,
            y: 0,
            url: "https://evil.example/\x1b]0;pwned\x07".into(),
            label: "x".into(),
        };
        let seq = format_osc8_span(&span).expect("controls stripped, scheme still ok");
        assert!(
            !seq.contains('\x07') && !seq.contains("\x1b]0"),
            "control breakout leaked: {seq:?}"
        );
        assert!(
            seq.contains("https://evil.example/pwned") || seq.contains("https://evil.example/")
        );
    }

    #[test]
    fn rejects_non_http_schemes() {
        let span = Osc8Span {
            x: 0,
            y: 0,
            url: "file:///etc/passwd".into(),
            label: "x".into(),
        };
        assert!(format_osc8_span(&span).is_none());
    }

    #[test]
    fn emit_drains_queue() {
        let mut buf = Vec::new();
        let mut spans = vec![Osc8Span {
            x: 0,
            y: 0,
            url: "https://a.test/".into(),
            label: "a".into(),
        }];
        emit_osc8_spans(&mut buf, &mut spans);
        assert!(spans.is_empty());
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("https://a.test/"));
    }
}
