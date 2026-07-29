//! Completion dropdown for slash commands and `@path` mentions.
//!
//! One menu component, multiple providers — so slash and path completion
//! share selection, rendering, and key handling instead of drifting apart
//! (#560). Filtered with [`crate::ui::fuzzy`] when the query is non-empty.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::colors::palette;

/// Max rows shown before a scroll window.
pub const MAX_VISIBLE_ROWS: usize = 6;
/// Cap for the label column.
const LABEL_CAP: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Slash,
    Path,
}

#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Left column (command name or path).
    pub label: String,
    /// Right column (description or empty).
    pub description: String,
    /// Text written into the composer on accept.
    pub insert: String,
}

#[derive(Debug, Clone)]
pub struct CompletionMenu {
    pub kind: CompletionKind,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
    /// Byte range in the composer this menu replaces on accept.
    pub replace_start: usize,
    pub replace_end: usize,
}

impl CompletionMenu {
    pub fn new(
        kind: CompletionKind,
        items: Vec<CompletionItem>,
        replace_start: usize,
        replace_end: usize,
    ) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        Some(Self {
            kind,
            items,
            selected: 0,
            replace_start,
            replace_end,
        })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn move_sel(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let n = self.items.len() as i32;
        let cur = self.selected as i32;
        self.selected = ((cur + delta).rem_euclid(n)) as usize;
    }

    pub fn current(&self) -> &CompletionItem {
        &self.items[self.selected.min(self.items.len() - 1)]
    }

    /// Visible window (scroll) so the selection stays on screen.
    pub fn window(&self) -> (usize, usize) {
        let n = self.items.len();
        if n <= MAX_VISIBLE_ROWS {
            return (0, n);
        }
        let half = MAX_VISIBLE_ROWS / 2;
        let mut start = self.selected.saturating_sub(half);
        if start + MAX_VISIBLE_ROWS > n {
            start = n - MAX_VISIBLE_ROWS;
        }
        (start, start + MAX_VISIBLE_ROWS)
    }
}

/// Build slash-command items for `partial` (without leading `/`).
pub fn slash_items(partial: &str) -> Vec<CompletionItem> {
    let q = partial.trim().trim_start_matches('/');
    // Prefer fuzzy palette listing when available; fall back to prefix complete.
    let ranked = crate::commands::list_slash_for_palette(q);
    if !ranked.is_empty() {
        return ranked
            .into_iter()
            .map(|(name, desc)| CompletionItem {
                label: format!("/{name}"),
                description: desc.to_string(),
                insert: format!("/{name} "),
            })
            .collect();
    }
    crate::commands::complete_slash(q)
        .into_iter()
        .map(|name| CompletionItem {
            label: format!("/{name}"),
            description: String::new(),
            insert: format!("/{name} "),
        })
        .collect()
}

/// Build path items for an `@` token.
pub fn path_items(cwd: &std::path::Path, partial: &str) -> Vec<CompletionItem> {
    let cands = super::mentions::complete_at_path(cwd, partial);
    let mut items: Vec<CompletionItem> = cands
        .into_iter()
        .map(|p| {
            let text = super::mentions::mention_text(&p);
            let is_dir = text.ends_with('/');
            CompletionItem {
                label: format!("@{text}"),
                description: if is_dir {
                    "directory".into()
                } else {
                    "file".into()
                },
                insert: format!("@{text}"),
            }
        })
        .collect();
    if !partial.is_empty() {
        items = crate::ui::fuzzy::fuzzy_rank(partial, items, |it| it.label.trim_start_matches('@'));
    }
    items
}

/// Paint the dropdown above `composer` (or clipped to `area`).
pub fn draw(frame: &mut Frame<'_>, area: Rect, menu: &CompletionMenu) {
    let (start, end) = menu.window();
    let rows = end - start;
    if rows == 0 {
        return;
    }
    let height = (rows as u16).saturating_add(2).min(area.height);
    let width = area.width.clamp(24, 72);
    // Sit just above the bottom of `area` (composer row).
    let y = area.y.saturating_add(area.height.saturating_sub(height));
    let x = area.x;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, rect);

    let p = palette();
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows);
    for (i, item) in menu.items[start..end].iter().enumerate() {
        let idx = start + i;
        let selected = idx == menu.selected;
        let prefix = if selected { "❯ " } else { "  " };
        let mut label = item.label.clone();
        if label.chars().count() > LABEL_CAP {
            label = label.chars().take(LABEL_CAP.saturating_sub(1)).collect();
            label.push('…');
        }
        let style = if selected {
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.text)
        };
        let desc_style = Style::default().fg(p.muted).add_modifier(Modifier::DIM);
        let mut spans = vec![
            Span::styled(prefix.to_string(), style),
            Span::styled(format!("{label:<width$}", width = LABEL_CAP.min(24)), style),
        ];
        if !item.description.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(item.description.clone(), desc_style));
        }
        lines.push(Line::from(spans));
    }

    let title = match menu.kind {
        CompletionKind::Slash => " commands ",
        CompletionKind::Path => " paths ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.muted))
        .title(Span::styled(title, Style::default().fg(p.accent)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_items_include_help() {
        let items = slash_items("hel");
        assert!(
            items.iter().any(|i| i.label == "/help"),
            "expected /help in {items:?}"
        );
    }

    #[test]
    fn move_sel_wraps() {
        let items = vec![
            CompletionItem {
                label: "/a".into(),
                description: String::new(),
                insert: "/a ".into(),
            },
            CompletionItem {
                label: "/b".into(),
                description: String::new(),
                insert: "/b ".into(),
            },
        ];
        let mut m = CompletionMenu::new(CompletionKind::Slash, items, 0, 1).unwrap();
        m.move_sel(1);
        assert_eq!(m.selected, 1);
        m.move_sel(1);
        assert_eq!(m.selected, 0);
        m.move_sel(-1);
        assert_eq!(m.selected, 1);
    }

    #[test]
    fn window_keeps_selection_visible() {
        let items: Vec<_> = (0..20)
            .map(|i| CompletionItem {
                label: format!("/{i}"),
                description: String::new(),
                insert: format!("/{i} "),
            })
            .collect();
        let mut m = CompletionMenu::new(CompletionKind::Slash, items, 0, 1).unwrap();
        m.selected = 15;
        let (s, e) = m.window();
        assert!(s <= 15 && 15 < e);
        assert_eq!(e - s, MAX_VISIBLE_ROWS);
    }
}
