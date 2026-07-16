//! Tool-call classification and grouping for typed cards (plan §M4).
//!
//! Tool calls are classified by name into a [`ToolKind`] so each renders as
//! a typed card (icon + label + kind-specific summary), and consecutive
//! read-only successes fold into a single group line. Grouping is a *view*
//! — the underlying transcript items are never destroyed.

use super::app::TranscriptItem;

/// The card category a tool name maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Bash,
    Read,
    Edit,
    Search,
    Fetch,
    Task,
    Mcp,
    Other,
}

impl ToolKind {
    /// Classify by tool name (matches the engine's tool identifiers).
    pub fn classify(name: &str) -> ToolKind {
        match name {
            "Bash" | "PowerShell" | "Shell" => ToolKind::Bash,
            "FileRead" | "Read" => ToolKind::Read,
            "FileWrite" | "FileEdit" | "MultiEdit" | "Edit" | "Write" => ToolKind::Edit,
            "Grep" | "Glob" | "Search" | "WebSearch" => ToolKind::Search,
            "WebFetch" | "Fetch" => ToolKind::Fetch,
            "Agent" | "Task" => ToolKind::Task,
            // MCP tools are namespaced "server.tool" or prefixed "mcp".
            n if n.contains('.') || n.starts_with("mcp") || n.starts_with("Mcp") => ToolKind::Mcp,
            _ => ToolKind::Other,
        }
    }

    /// Leading glyph for the card header.
    pub fn icon(self) -> &'static str {
        match self {
            ToolKind::Bash => "⚡",
            ToolKind::Read => "📄",
            ToolKind::Edit => "✏",
            ToolKind::Search => "🔍",
            ToolKind::Fetch => "🌐",
            ToolKind::Task => "🤖",
            ToolKind::Mcp => "🔌",
            ToolKind::Other => "•",
        }
    }

    /// Short lowercase label for the card header.
    pub fn label(self) -> &'static str {
        match self {
            ToolKind::Bash => "bash",
            ToolKind::Read => "read",
            ToolKind::Edit => "edit",
            ToolKind::Search => "search",
            ToolKind::Fetch => "fetch",
            ToolKind::Task => "task",
            ToolKind::Mcp => "mcp",
            ToolKind::Other => "tool",
        }
    }

    /// Read-only kinds are eligible for consecutive-success grouping.
    pub fn is_read_only(self) -> bool {
        matches!(self, ToolKind::Read | ToolKind::Search | ToolKind::Fetch)
    }
}

/// True if a transcript item is a completed, successful, read-only tool
/// call — the only kind eligible to fold into a read group.
pub fn is_groupable(item: &TranscriptItem) -> bool {
    matches!(
        item,
        TranscriptItem::Tool { name, result: Some(_), is_error: false, .. }
            if ToolKind::classify(name).is_read_only()
    )
}

/// Minimum consecutive groupable cards before they fold (plan §M4).
pub const MIN_GROUP: usize = 3;

/// A block to display: either a single transcript item or a folded run of
/// consecutive groupable tool cards (by index into the transcript).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Display {
    Single(usize),
    Group(Vec<usize>),
}

/// Fold the transcript into display blocks, grouping runs of ≥[`MIN_GROUP`]
/// consecutive groupable cards. A running or failed card breaks the run.
pub fn plan_display(items: &[TranscriptItem]) -> Vec<Display> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if is_groupable(&items[i]) {
            let start = i;
            while i < items.len() && is_groupable(&items[i]) {
                i += 1;
            }
            let run: Vec<usize> = (start..i).collect();
            if run.len() >= MIN_GROUP {
                out.push(Display::Group(run));
            } else {
                out.extend(run.into_iter().map(Display::Single));
            }
        } else {
            out.push(Display::Single(i));
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(detail: &str) -> TranscriptItem {
        TranscriptItem::Tool {
            call_id: String::new(),
            name: "FileRead".into(),
            detail: detail.into(),
            result: Some("ok".into()),
            is_error: false,
            live: None,
        }
    }
    fn bash() -> TranscriptItem {
        TranscriptItem::Tool {
            call_id: String::new(),
            name: "Bash".into(),
            detail: "ls".into(),
            result: Some("ok".into()),
            is_error: false,
            live: None,
        }
    }
    fn running_read() -> TranscriptItem {
        TranscriptItem::Tool {
            call_id: String::new(),
            name: "FileRead".into(),
            detail: "x".into(),
            result: None,
            is_error: false,
            live: None,
        }
    }

    #[test]
    fn classify_maps_names() {
        assert_eq!(ToolKind::classify("Bash"), ToolKind::Bash);
        assert_eq!(ToolKind::classify("FileRead"), ToolKind::Read);
        assert_eq!(ToolKind::classify("MultiEdit"), ToolKind::Edit);
        assert_eq!(ToolKind::classify("Grep"), ToolKind::Search);
        assert_eq!(ToolKind::classify("WebFetch"), ToolKind::Fetch);
        assert_eq!(ToolKind::classify("Agent"), ToolKind::Task);
        assert_eq!(ToolKind::classify("github.create_issue"), ToolKind::Mcp);
        assert_eq!(ToolKind::classify("Weird"), ToolKind::Other);
    }

    #[test]
    fn three_consecutive_reads_group() {
        let items = vec![read("a"), read("b"), read("c")];
        assert_eq!(plan_display(&items), vec![Display::Group(vec![0, 1, 2])]);
    }

    #[test]
    fn two_reads_do_not_group() {
        let items = vec![read("a"), read("b")];
        assert_eq!(
            plan_display(&items),
            vec![Display::Single(0), Display::Single(1)]
        );
    }

    #[test]
    fn a_bash_between_reads_breaks_the_group() {
        let items = vec![read("a"), read("b"), bash(), read("c"), read("d")];
        // Neither run reaches 3, so nothing folds.
        assert_eq!(
            plan_display(&items),
            vec![
                Display::Single(0),
                Display::Single(1),
                Display::Single(2),
                Display::Single(3),
                Display::Single(4),
            ]
        );
    }

    #[test]
    fn running_card_breaks_the_group() {
        let items = vec![read("a"), read("b"), running_read(), read("c")];
        assert!(
            plan_display(&items)
                .iter()
                .all(|d| matches!(d, Display::Single(_))),
            "a running read must not fold"
        );
    }

    #[test]
    fn group_then_single() {
        let items = vec![read("a"), read("b"), read("c"), bash()];
        assert_eq!(
            plan_display(&items),
            vec![Display::Group(vec![0, 1, 2]), Display::Single(3)]
        );
    }
}
