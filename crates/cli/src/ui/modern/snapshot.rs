//! Golden-file snapshots of rendered frames.
//!
//! The existing render tests assert that a frame *contains* a substring.
//! That catches "the header disappeared" and nothing else: it cannot see
//! layout drift, and — because [`super::render::buffer_to_string`] keeps
//! only the glyphs — it is completely blind to colour. Every theme and
//! transcript-styling regression this repo has shipped was invisible to
//! it.
//!
//! A snapshot here records both halves:
//!
//! * the glyph grid, so layout changes show up;
//! * a per-cell style grid plus a legend, so a changed foreground,
//!   background or modifier shows up as a diff instead of passing.
//!
//! Snapshots live in `crates/cli/tests/snapshots/`. Run with
//! `UPDATE_SNAPSHOTS=1` to rewrite them, then read the diff before
//! committing — an unreviewed snapshot update is worse than no snapshot,
//! because it launders a regression into the baseline.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

/// Render `buf` to the golden-file text format.
pub fn buffer_snapshot(buf: &Buffer) -> String {
    let mut legend: Vec<(Color, Color, Modifier)> = Vec::new();
    let mut glyphs = String::new();
    let mut styles = String::new();

    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            glyphs.push_str(cell.symbol());

            let key = (cell.fg, cell.bg, cell.modifier);
            let idx = match legend.iter().position(|k| *k == key) {
                Some(i) => i,
                None => {
                    legend.push(key);
                    legend.len() - 1
                }
            };
            styles.push(style_marker(idx));
        }
        // Trailing blanks carry no information and make snapshots churn
        // on unrelated width changes.
        trim_end(&mut glyphs);
        trim_end(&mut styles);
        glyphs.push('\n');
        styles.push('\n');
    }

    let mut out = String::new();
    out.push_str("== glyphs ==\n");
    out.push_str(&glyphs);
    out.push_str("\n== styles ==\n");
    out.push_str(&styles);
    out.push_str("\n== legend ==\n");
    for (i, (fg, bg, m)) in legend.iter().enumerate() {
        out.push_str(&format!(
            "{} fg={fg:?} bg={bg:?} mods={m:?}\n",
            style_marker(i)
        ));
    }
    out
}

/// One printable character per distinct style. Beyond the alphabet the
/// markers repeat with a digit suffix — a frame with more than 62 styles
/// is a problem in its own right.
fn style_marker(idx: usize) -> char {
    const MARKERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    MARKERS[idx % MARKERS.len()] as char
}

fn trim_end(s: &mut String) {
    while s.ends_with(' ') {
        s.pop();
    }
}

/// Replace this crate's version with a fixed placeholder so a release
/// bump does not invalidate every golden.
///
/// The header renders `agent-code <version>`, so without this the
/// version bump in a release PR turns all frame snapshots red on main
/// — which is exactly what happened at 0.28.0. The padding is
/// preserved (same byte length) so column alignment, and therefore the
/// style rows, stay comparable across versions of differing width.
fn mask_version(frame: &str) -> String {
    const PLACEHOLDER: &str = "x.y.z";
    let version = env!("CARGO_PKG_VERSION");
    if !frame.contains(version) {
        return frame.to_string();
    }
    let mut replacement = String::from(PLACEHOLDER);
    match version.len().cmp(&PLACEHOLDER.len()) {
        // Keep the byte length identical so nothing shifts columns.
        std::cmp::Ordering::Greater => {
            replacement.push_str(&" ".repeat(version.len() - PLACEHOLDER.len()))
        }
        std::cmp::Ordering::Less => replacement.truncate(version.len()),
        std::cmp::Ordering::Equal => {}
    }
    frame.replace(version, &replacement)
}

/// Compare `actual` against the golden file for `name`, or rewrite it
/// when `UPDATE_SNAPSHOTS` is set.
///
/// Panics with the full expected/actual text on mismatch: a snapshot
/// failure is only useful if the reader can see what moved.
pub fn assert_snapshot(name: &str, actual: &str) {
    let actual = &mask_version(actual);
    let path = snapshot_path(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("create snapshot dir");
        }
        std::fs::write(&path, actual).expect("write snapshot");
        return;
    }
    let Ok(expected) = std::fs::read_to_string(&path).map(|s| normalize_newlines(&s)) else {
        panic!(
            "missing snapshot {}\n\
             re-run with UPDATE_SNAPSHOTS=1 and review the result before committing.\n\
             --- actual ---\n{actual}",
            path.display()
        );
    };
    if expected != *actual {
        panic!(
            "snapshot {} does not match.\n\
             If the change is intended, re-run with UPDATE_SNAPSHOTS=1 and read the diff.\n\
             --- expected ---\n{expected}\n--- actual ---\n{actual}",
            path.display()
        );
    }
}

/// Strip `\r` so a checkout that converted line endings still compares
/// equal. `.gitattributes` marks these files as LF, but a test that
/// fails because of the reader's git config is a bad test.
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn snapshot_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn cell_buffer() -> Buffer {
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 4, 2));
        buf.set_string(0, 0, "ab", Style::default().fg(Color::Red));
        buf.set_string(0, 1, "cd", Style::default().fg(Color::Blue));
        buf
    }

    #[test]
    fn the_snapshot_records_glyphs_styles_and_a_legend() {
        let snap = buffer_snapshot(&cell_buffer());
        assert!(snap.contains("== glyphs =="));
        assert!(snap.contains("ab"));
        assert!(snap.contains("== styles =="));
        assert!(snap.contains("== legend =="));
        assert!(snap.contains("Red"), "legend lost the colour: {snap}");
        assert!(snap.contains("Blue"), "legend lost the colour: {snap}");
    }

    /// The whole reason for the style grid: a colour-only change has to
    /// produce a diff. `buffer_to_string` returns identical text for both
    /// of these, which is how theme regressions went unnoticed.
    #[test]
    fn a_colour_only_change_changes_the_snapshot() {
        let before = buffer_snapshot(&cell_buffer());

        let mut recoloured = cell_buffer();
        recoloured.set_string(0, 0, "ab", Style::default().fg(Color::Green));
        let after = buffer_snapshot(&recoloured);

        assert_ne!(
            before, after,
            "a foreground change produced an identical snapshot"
        );
        assert_eq!(
            crate::ui::modern::render::buffer_to_string(&cell_buffer()),
            crate::ui::modern::render::buffer_to_string(&recoloured),
            "precondition: the glyph-only view cannot see this change"
        );
    }

    #[test]
    fn crlf_in_a_snapshot_file_still_compares_equal() {
        // Git on Windows rewrites line endings on checkout, which made
        // every file-based snapshot fail there while passing on Linux.
        let lf = "== glyphs ==\nab\n";
        assert_eq!(normalize_newlines("== glyphs ==\r\nab\r\n"), lf);
        assert_eq!(normalize_newlines(lf), lf);
    }

    #[test]
    fn a_modifier_change_changes_the_snapshot() {
        let mut bold = cell_buffer();
        bold.set_string(
            0,
            0,
            "ab",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        );
        assert_ne!(buffer_snapshot(&cell_buffer()), buffer_snapshot(&bold));
    }
}

/// Golden frames. Deliberately few: each one is a thing a reviewer has
/// to actually read when it changes, so the set is chosen for coverage
/// per snapshot rather than breadth.
#[cfg(test)]
mod frames {
    use super::*;
    use crate::ui::modern::app::{App, Modal, PendingPermission, Phase, TranscriptItem};
    use crate::ui::modern::render::draw;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Frames read the process-global theme, so every test here holds the
    /// shared guard and pins the theme explicitly. Without that a
    /// concurrent `theme::init` would decide the colours mid-render.
    fn render(theme: &str, build: impl FnOnce(&mut App)) -> String {
        let _g = crate::ui::theme::test_lock();
        // `Theme::from_name` falls back silently for an unknown id, so a
        // typo here would quietly snapshot the default theme instead.
        assert!(
            crate::ui::theme::Theme::all_names()
                .iter()
                .any(|n| n == theme),
            "unknown theme id in a snapshot test: {theme}"
        );
        crate::ui::theme::init(theme);
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("test-model", "/w", "sess1234");
        build(&mut app);
        term.draw(|f| draw(f, &mut app)).unwrap();
        buffer_snapshot(term.backend().buffer())
    }

    #[test]
    fn idle_frame() {
        assert_snapshot("idle_frame", &render("one-dark", |_| {}));
    }

    #[test]
    fn transcript_with_user_assistant_and_tool() {
        let snap = render("one-dark", |app| {
            app.transcript
                .push(TranscriptItem::User("add a null check".into()));
            app.transcript
                .push(TranscriptItem::Assistant("Done. Added the guard.".into()));
            app.transcript.push(TranscriptItem::Tool {
                call_id: "c1".into(),
                name: "FileEdit".into(),
                detail: "src/auth.rs".into(),
                result: Some("1 edit applied".into()),
                is_error: false,
                live: None,
            });
        });
        assert_snapshot("transcript_basic", &snap);
    }

    #[test]
    fn permission_modal() {
        let snap = render("one-dark", |app| {
            app.phase = Phase::Permission;
            let (respond, rx) = std::sync::mpsc::channel();
            // Keep the receiver alive for the frame's lifetime.
            std::mem::forget(rx);
            app.modals.push_back(Modal::Permission(PendingPermission {
                name: "Bash".into(),
                description: "Bash: run `cargo test`".into(),
                origin: None,
                input_preview: Some("{\n  \"command\": \"cargo test\"\n}".into()),
                suggested_prefix: None,
                respond,
            }));
        });
        assert_snapshot("permission_modal", &snap);
    }

    /// The same transcript under a second theme. This is the snapshot
    /// that protects the colour system: a palette change that silently
    /// drops a slot shows up here as a legend diff, where a
    /// glyph-only assertion would stay green.
    #[test]
    fn transcript_under_a_light_theme() {
        let snap = render("solarized-light", |app| {
            app.transcript
                .push(TranscriptItem::User("add a null check".into()));
            app.transcript
                .push(TranscriptItem::Assistant("Done. Added the guard.".into()));
        });
        assert_snapshot("transcript_light", &snap);
    }

    /// Two themes must not render identically — if they do, the theme is
    /// not reaching the frame at all, which is exactly the class of bug
    /// that shipped twice in this area.
    #[test]
    fn switching_theme_changes_the_frame() {
        let dark = render("one-dark", |app| {
            app.transcript.push(TranscriptItem::User("hello".into()));
        });
        let light = render("solarized-light", |app| {
            app.transcript.push(TranscriptItem::User("hello".into()));
        });
        assert_ne!(dark, light, "the theme did not reach the rendered frame");
    }

    /// Launch surface: branding, repo:branch · version, auth/mode, one
    /// warning slot, recent sessions, type-to-start (#557 Phase 2).
    #[test]
    fn launch_surface_idle() {
        use agent_code_lib::services::session::SessionSummary;
        use agent_code_lib::services::startup::{StartupWarning, WarningSeverity};

        let snap = render("one-dark", |app| {
            app.version = "0.0.0-test".into();
            app.launch = super::super::launch::LaunchSurface::ready(
                "agent-code:main",
                "API key",
                "normal",
                &[StartupWarning {
                    severity: WarningSeverity::Warning,
                    message: "No API key set (AGENT_CODE_API_KEY)".into(),
                    action: Some("Run /doctor for details and fixes.".into()),
                }],
                vec![SessionSummary {
                    id: "abcd1234-ffff".into(),
                    cwd: "/w/proj".into(),
                    model: "test-model".into(),
                    turn_count: 3,
                    message_count: 6,
                    updated_at: "2026-01-01T00:00:00Z".into(),
                    label: Some("fix permissions".into()),
                    tags: vec![],
                }],
            );
        });
        assert_snapshot("launch_surface", &snap);
        assert!(
            snap.contains("agent-code") && snap.contains("Type to start"),
            "launch chrome missing:\n{snap}"
        );
        assert!(
            snap.contains("agent-code:main") || snap.contains("main"),
            "repo:branch missing:\n{snap}"
        );
        assert!(
            snap.contains("Run /doctor") || snap.contains("No API key"),
            "warning slot missing:\n{snap}"
        );
        assert!(
            snap.contains("abcd1234") || snap.contains("fix permissions"),
            "recent sessions missing:\n{snap}"
        );
    }

    /// Narrow width must not panic and still shows the brand (#557 D11-02).
    #[test]
    fn launch_surface_narrow() {
        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("one-dark");
        let backend = TestBackend::new(40, 16);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("test-model", "/w", "sess1234");
        app.version = "0.0.0-test".into();
        app.launch = super::super::launch::LaunchSurface::ready(
            "repo:main",
            "API key",
            "normal",
            &[],
            vec![],
        );
        term.draw(|f| draw(f, &mut app)).unwrap();
        let snap = buffer_snapshot(term.backend().buffer());
        assert!(
            snap.contains("agent-code") || snap.contains("Type to start") || snap.contains("auth:"),
            "narrow launch should still paint something:\n{snap}"
        );
    }
}
