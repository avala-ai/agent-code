//! Launch surface — the front door before the first turn.
//!
//! Phases 2–3 of the first-run / launch epic (#557): branding, `repo:branch ·
//! version`, auth + permission mode, a single startup-warning slot, recent
//! sessions with ↑/↓ + Enter (or click) to resume, brand shimmer gated by
//! `[ui] reduced_motion`, and a type-to-start prompt.

use agent_code_lib::services::session::SessionSummary;
use agent_code_lib::services::startup::{self, StartupWarning, WarningSeverity};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::anim::shimmer_style;
use super::app::{App, Phase, TranscriptItem};
use super::colors::palette;
use super::hit_rect::HitTarget;

/// How many recent sessions to list on the launch screen.
pub const RECENT_LIMIT: usize = 5;

/// State for the launch / welcome surface.
///
/// Defaults to hidden (`visible: false`) so unit tests and non-interactive
/// surfaces stay off until [`LaunchSurface::ready`] populates one.
#[derive(Debug, Clone, Default)]
pub struct LaunchSurface {
    /// When false the surface is gone for the rest of the process.
    pub visible: bool,
    /// `repo:branch` (or a path basename when not a git checkout).
    pub repo_branch: String,
    /// Auth summary, e.g. "API key" / "xAI OAuth" / "no credentials".
    pub auth_label: String,
    /// Session mode label, e.g. "normal" / "plan" (from [`super::mode::SessionMode`]).
    pub mode_label: String,
    /// Single-slot banner (already filtered by [`startup::pick_banner`]).
    pub warning: Option<StartupWarning>,
    /// Recent sessions, newest first (capped at [`RECENT_LIMIT`]).
    pub recent: Vec<SessionSummary>,
    /// Highlighted row in [`Self::recent`] (↑/↓).
    pub selected: usize,
}

impl LaunchSurface {
    /// Build a visible launch surface from already-collected data.
    pub fn ready(
        repo_branch: impl Into<String>,
        auth_label: impl Into<String>,
        mode_label: impl Into<String>,
        warnings: &[StartupWarning],
        recent: Vec<SessionSummary>,
    ) -> Self {
        let mut recent = recent;
        recent.truncate(RECENT_LIMIT);
        Self {
            visible: true,
            repo_branch: repo_branch.into(),
            auth_label: auth_label.into(),
            mode_label: mode_label.into(),
            warning: startup::pick_banner(warnings).cloned(),
            recent,
            selected: 0,
        }
    }

    /// Hide the surface permanently for this process.
    pub fn dismiss(&mut self) {
        self.visible = false;
    }

    /// True when the launch surface should paint over the empty transcript.
    pub fn should_draw(&self, app: &App) -> bool {
        self.visible
            && app.phase == Phase::Idle
            && !app.session_picker_open()
            && !app.model_picker_open()
            && !app.theme_picker_open()
            && !app.command_palette_open()
            && transcript_is_fresh(&app.transcript)
    }

    /// Move the recent-session highlight. No-op when the list is empty.
    pub fn move_selection(&mut self, delta: i32) {
        let n = self.recent.len();
        if n == 0 {
            return;
        }
        let cur = self.selected.min(n - 1) as i32;
        let next = (cur + delta).rem_euclid(n as i32) as usize;
        self.selected = next;
    }

    /// Id of the highlighted recent session, if any.
    pub fn highlighted_id(&self) -> Option<&str> {
        self.recent
            .get(self.selected.min(self.recent.len().saturating_sub(1)))
            .map(|s| s.id.as_str())
            .filter(|_| !self.recent.is_empty())
    }
}

/// True when the transcript has no user/assistant work — only chrome
/// system lines (or nothing). Matches the empty-guidance predicate so
/// the launch surface and later empty-state share the same trigger.
pub fn transcript_is_fresh(items: &[TranscriptItem]) -> bool {
    !items.iter().any(|i| {
        matches!(
            i,
            TranscriptItem::User(_)
                | TranscriptItem::Assistant(_)
                | TranscriptItem::Thinking { .. }
                | TranscriptItem::Tool { .. }
        )
    })
}

/// Auth summary for the launch chrome.
pub fn auth_label(config: &agent_code_lib::config::Config) -> String {
    use agent_code_lib::config::ApiAuthMode;
    match config.api.auth_mode {
        ApiAuthMode::CodexChatgpt => "Codex ChatGPT".into(),
        ApiAuthMode::XaiOauth => "xAI OAuth".into(),
        ApiAuthMode::ApiKey => {
            if config.api.api_key.is_some() {
                "API key".into()
            } else {
                "no credentials".into()
            }
        }
    }
}

/// `basename:branch` or just the basename when git is unavailable.
pub fn repo_branch_label(cwd: &str, repo_root: Option<&str>, branch: Option<&str>) -> String {
    let base = repo_root
        .map(std::path::Path::new)
        .or_else(|| Some(std::path::Path::new(cwd)))
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(cwd);
    match branch {
        Some(b) if !b.is_empty() => format!("{base}:{b}"),
        _ => base.to_string(),
    }
}

/// Paint the launch surface into `area` (the transcript body).
///
/// Registers [`HitTarget::LaunchRecent`] rows so clicks resume a session.
pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let launch = &app.launch;
    if area.height < 4 || area.width < 20 {
        return;
    }
    let p = palette();
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Absolute line index within `lines` for each recent row (for hit rects).
    let mut recent_line_idxs: Vec<(usize, usize)> = Vec::new(); // (line_idx, recent_i)

    // Brand (shimmer when motion is allowed)
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  agent-code",
        shimmer_style(app.tick, app.reduced_motion, p.accent),
    )));

    // repo:branch · version
    let mut meta = launch.repo_branch.clone();
    if !app.version.is_empty() {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push('v');
        meta.push_str(&app.version);
    }
    if !meta.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {meta}"),
            Style::default().fg(p.muted),
        )));
    }

    // Live mode so Shift+Tab updates the line without rebuilding the
    // surface (Phase 3 will lean on this).
    let mode = app.mode.label();
    let auth = if launch.auth_label.is_empty() {
        "—"
    } else {
        launch.auth_label.as_str()
    };
    lines.push(Line::from(Span::styled(
        format!("  auth: {auth} · mode: {mode}"),
        Style::default().fg(p.muted),
    )));

    // Warning banner (single slot)
    if let Some(ref w) = launch.warning {
        lines.push(Line::from(""));
        let (fg, tag) = match w.severity {
            WarningSeverity::Warning => (p.warning, "!"),
            WarningSeverity::Info => (p.muted, "i"),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {tag} "),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(w.message.clone(), Style::default().fg(fg)),
        ]));
        if let Some(ref action) = w.action {
            lines.push(Line::from(Span::styled(
                format!("    {action}"),
                Style::default().fg(p.muted).add_modifier(Modifier::DIM),
            )));
        }
    }

    // Recent sessions
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Recent",
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )));
    if launch.recent.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (none yet — they appear after you chat)",
            Style::default().fg(p.muted).add_modifier(Modifier::DIM),
        )));
    } else {
        let sel = launch.selected.min(launch.recent.len() - 1);
        for (i, s) in launch.recent.iter().enumerate() {
            let id = short_id(&s.id);
            let label = s
                .label
                .as_deref()
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| short_cwd(&s.cwd));
            let turns = if s.turn_count == 1 {
                "1 turn".to_string()
            } else {
                format!("{} turns", s.turn_count)
            };
            let marker = if i == sel { "›" } else { " " };
            let row = format!("  {marker} {id}  {turns}  {label}");
            let row = truncate_cols(&row, area.width.saturating_sub(2) as usize);
            let style = if i == sel {
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(p.inactive)
            };
            recent_line_idxs.push((lines.len(), i));
            lines.push(Line::from(Span::styled(row, style)));
        }
        lines.push(Line::from(Span::styled(
            "    ↑/↓ or click · Enter resume · type to start",
            Style::default().fg(p.muted).add_modifier(Modifier::DIM),
        )));
    }

    // Type-to-start
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Type to start a conversation",
        Style::default().fg(p.text),
    )));
    lines.push(Line::from(Span::styled(
        "  Shift+Tab mode · Ctrl+P commands · ? shortcuts",
        Style::default().fg(p.muted).add_modifier(Modifier::DIM),
    )));

    // Centre vertically when the pane is tall enough.
    let h = lines.len() as u16;
    let y = if area.height > h {
        area.y + area.height.saturating_sub(h) / 2
    } else {
        area.y
    };
    let rect = Rect {
        x: area.x,
        y,
        width: area.width,
        height: h.min(area.height),
    };
    // Hit-test each recent row (absolute screen y = origin + line index).
    for (line_idx, recent_i) in recent_line_idxs {
        let row_y = y.saturating_add(line_idx as u16);
        if row_y >= area.y.saturating_add(area.height) {
            break;
        }
        app.hit_registry.register(
            Rect {
                x: area.x,
                y: row_y,
                width: area.width,
                height: 1,
            },
            HitTarget::LaunchRecent { index: recent_i },
        );
    }
    frame.render_widget(Paragraph::new(lines), rect);
}

fn short_id(id: &str) -> String {
    let take = id.chars().take(8).collect::<String>();
    if take.is_empty() {
        "????????".into()
    } else {
        take
    }
}

fn short_cwd(cwd: &str) -> &str {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cwd)
}

fn truncate_cols(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    if max == 1 {
        return "…".into();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_code_lib::services::startup::{StartupWarning, WarningSeverity};

    #[test]
    fn fresh_transcript_ignores_system_chrome() {
        assert!(transcript_is_fresh(&[]));
        assert!(transcript_is_fresh(&[TranscriptItem::System(
            "Modern TUI · hello".into()
        )]));
        assert!(!transcript_is_fresh(&[TranscriptItem::User("hi".into())]));
        assert!(!transcript_is_fresh(&[TranscriptItem::Assistant(
            "yo".into()
        )]));
    }

    #[test]
    fn dismiss_hides_permanently() {
        let mut l = LaunchSurface::ready("repo:main", "API key", "normal", &[], vec![]);
        assert!(l.visible);
        l.dismiss();
        assert!(!l.visible);
    }

    #[test]
    fn ready_picks_warning_banner() {
        let warnings = vec![
            StartupWarning {
                severity: WarningSeverity::Info,
                message: "info".into(),
                action: None,
            },
            StartupWarning {
                severity: WarningSeverity::Warning,
                message: "broken".into(),
                action: Some("fix it".into()),
            },
        ];
        let l = LaunchSurface::ready("r:b", "API key", "normal", &warnings, vec![]);
        assert_eq!(
            l.warning.as_ref().map(|w| w.message.as_str()),
            Some("broken")
        );
    }

    #[test]
    fn repo_branch_label_formats() {
        assert_eq!(
            repo_branch_label(
                "/home/u/agent-code",
                Some("/home/u/agent-code"),
                Some("main")
            ),
            "agent-code:main"
        );
        assert_eq!(repo_branch_label("/tmp/proj", None, None), "proj");
    }

    #[test]
    fn recent_is_capped() {
        let rows: Vec<SessionSummary> = (0..10)
            .map(|i| SessionSummary {
                id: format!("id{i}"),
                cwd: "/w".into(),
                model: "m".into(),
                turn_count: 1,
                message_count: 2,
                updated_at: "t".into(),
                label: None,
                tags: vec![],
            })
            .collect();
        let l = LaunchSurface::ready("r", "a", "n", &[], rows);
        assert_eq!(l.recent.len(), RECENT_LIMIT);
    }

    #[test]
    fn typing_dismisses_launch_surface() {
        let mut app = App::new("m", "/w", "s");
        app.launch = LaunchSurface::ready("r:b", "API key", "normal", &[], vec![]);
        assert!(app.launch.visible);
        app.insert_char('h');
        assert!(!app.launch.visible);
        assert_eq!(app.input, "h");
    }

    #[test]
    fn paste_dismisses_launch_surface() {
        let mut app = App::new("m", "/w", "s");
        app.launch = LaunchSurface::ready("r:b", "API key", "normal", &[], vec![]);
        app.insert_str("hello");
        assert!(!app.launch.visible);
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn palette_accept_dismisses_launch_surface() {
        let mut app = App::new("m", "/w", "s");
        app.launch = LaunchSurface::ready("r:b", "API key", "normal", &[], vec![]);
        app.open_command_palette();
        for c in "help".chars() {
            app.palette_insert_char(c);
        }
        app.palette_accept();
        assert!(
            !app.launch.visible,
            "palette fill must dismiss launch (no insert_char)"
        );
        assert!(app.input.starts_with("/help"));
    }

    #[test]
    fn slash_submit_dismisses_launch_surface() {
        let mut app = App::new("m", "/w", "s");
        app.launch = LaunchSurface::ready("r:b", "API key", "normal", &[], vec![]);
        // Simulate palette-filled input without going through insert_char.
        app.input = "/help".into();
        app.cursor = app.input.len();
        assert!(app.launch.visible);
        app.submit();
        assert!(
            !app.launch.visible,
            "slash submit must dismiss launch so System output is visible"
        );
    }

    fn summary(id: &str, label: Option<&str>) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            cwd: "/w".into(),
            model: "m".into(),
            turn_count: 2,
            message_count: 4,
            updated_at: "t".into(),
            label: label.map(str::to_owned),
            tags: vec![],
        }
    }

    #[test]
    fn move_selection_wraps() {
        let mut l = LaunchSurface::ready(
            "r",
            "a",
            "n",
            &[],
            vec![summary("a", None), summary("b", None), summary("c", None)],
        );
        assert_eq!(l.highlighted_id(), Some("a"));
        l.move_selection(1);
        assert_eq!(l.highlighted_id(), Some("b"));
        l.move_selection(1);
        assert_eq!(l.highlighted_id(), Some("c"));
        l.move_selection(1);
        assert_eq!(l.highlighted_id(), Some("a"));
        l.move_selection(-1);
        assert_eq!(l.highlighted_id(), Some("c"));
    }

    #[test]
    fn launch_accept_starts_resume_and_dismisses() {
        let mut app = App::new("m", "/w", "current");
        app.launch = LaunchSurface::ready(
            "r",
            "a",
            "n",
            &[],
            vec![summary("other-sess", Some("past work"))],
        );
        assert!(app.launch.visible);
        app.launch_accept();
        assert!(!app.launch.visible);
        assert!(
            app.status_message.contains("resuming"),
            "status: {}",
            app.status_message
        );
        // Gate must be armed for the run loop.
        assert!(app.resume.settle().is_some());
    }

    #[test]
    fn launch_accept_same_session_is_noop() {
        let mut app = App::new("m", "/w", "same-id");
        app.launch = LaunchSurface::ready("r", "a", "n", &[], vec![summary("same-id", None)]);
        app.launch_accept();
        assert!(app.status_message.contains("already in this session"));
        assert!(app.resume.settle().is_none());
    }

    #[test]
    fn draw_registers_launch_recent_hit_targets() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;

        let _g = crate::ui::theme::test_lock();
        crate::ui::theme::init("one-dark");
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let mut app = App::new("m", "/w", "s");
        app.launch = LaunchSurface::ready(
            "r:b",
            "API key",
            "normal",
            &[],
            vec![
                summary("sess-aaa", Some("one")),
                summary("sess-bbb", Some("two")),
            ],
        );
        term.draw(|f| {
            let area = Rect {
                x: 0,
                y: 3,
                width: 80,
                height: 14,
            };
            draw(f, area, &mut app);
        })
        .unwrap();
        let hits: Vec<_> = (0..24u16)
            .filter_map(|y| {
                app.hit_registry.hit_test(10, y).and_then(|t| match t {
                    HitTarget::LaunchRecent { index } => Some(*index),
                    _ => None,
                })
            })
            .collect();
        assert!(
            hits.contains(&0) && hits.contains(&1),
            "expected LaunchRecent hit targets for both rows, got {hits:?}"
        );
    }
}
