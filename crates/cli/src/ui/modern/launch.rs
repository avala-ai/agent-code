//! Launch surface — the front door before the first turn.
//!
//! Phase 2 of the first-run / launch epic (#557): branding, `repo:branch ·
//! version`, auth + permission mode, a single startup-warning slot, recent
//! sessions, and a type-to-start prompt. Interaction (arrow/Enter into a
//! recent session, mouse hit-testing, shimmer) is Phase 3.

use agent_code_lib::services::session::SessionSummary;
use agent_code_lib::services::startup::{self, StartupWarning, WarningSeverity};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{App, Phase, TranscriptItem};
use super::colors::palette;

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
pub fn draw(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let launch = &app.launch;
    if area.height < 4 || area.width < 20 {
        return;
    }
    let p = palette();
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Brand
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  agent-code",
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
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
        for s in &launch.recent {
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
            let row = format!("    {id}  {turns}  {label}");
            let row = truncate_cols(&row, area.width.saturating_sub(2) as usize);
            lines.push(Line::from(Span::styled(
                row,
                Style::default().fg(p.inactive),
            )));
        }
        lines.push(Line::from(Span::styled(
            "    /resume or Ctrl+P to open a past session",
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
}
