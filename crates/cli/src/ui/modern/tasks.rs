//! Subagent/tasks tracking for the tasks pane (plan §M8).
//!
//! `on_subagent_update` events maintain a list of [`TaskEntry`]. The pane
//! orders them Needs-input → Working → Done/Failed (Claude's agents-view
//! ordering) and shows a one-line headline per agent.

/// Lifecycle state of a tracked subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Working,
    NeedsInput,
    Done,
    Failed,
}

impl TaskState {
    /// Map an engine state string to a [`TaskState`] (defaults to Working).
    pub fn parse(s: &str) -> TaskState {
        match s.trim().to_ascii_lowercase().as_str() {
            "done" | "completed" | "complete" | "success" => TaskState::Done,
            "failed" | "error" | "killed" => TaskState::Failed,
            "needs_input" | "needs input" | "waiting" | "blocked" => TaskState::NeedsInput,
            _ => TaskState::Working,
        }
    }

    /// Section sort key: Needs-input first, then Working, then finished.
    pub fn order(self) -> u8 {
        match self {
            TaskState::NeedsInput => 0,
            TaskState::Working => 1,
            TaskState::Done => 2,
            TaskState::Failed => 3,
        }
    }

    /// Status glyph for the row.
    pub fn glyph(self) -> &'static str {
        match self {
            TaskState::Working => "●",
            TaskState::NeedsInput => "◐",
            TaskState::Done => "✓",
            TaskState::Failed => "✗",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            TaskState::Working => "working",
            TaskState::NeedsInput => "needs input",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
        }
    }
}

/// One tracked subagent.
#[derive(Debug, Clone)]
pub struct TaskEntry {
    pub agent_id: String,
    pub state: TaskState,
    pub headline: String,
}

/// Upsert a subagent update into `tasks`, keeping entries ordered by state
/// (needs-input → working → done/failed) with a stable within-section order.
pub fn upsert(tasks: &mut Vec<TaskEntry>, agent_id: &str, state: &str, headline: &str) {
    let state = TaskState::parse(state);
    if let Some(existing) = tasks.iter_mut().find(|t| t.agent_id == agent_id) {
        existing.state = state;
        if !headline.is_empty() {
            existing.headline = headline.to_string();
        }
    } else {
        tasks.push(TaskEntry {
            agent_id: agent_id.to_string(),
            state,
            headline: headline.to_string(),
        });
    }
    // Stable sort by section so needs-input rows float to the top.
    tasks.sort_by_key(|t| t.state.order());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_states() {
        assert_eq!(TaskState::parse("working"), TaskState::Working);
        assert_eq!(TaskState::parse("done"), TaskState::Done);
        assert_eq!(TaskState::parse("failed"), TaskState::Failed);
        assert_eq!(TaskState::parse("needs input"), TaskState::NeedsInput);
        assert_eq!(TaskState::parse("whatever"), TaskState::Working);
    }

    #[test]
    fn upsert_updates_existing_and_reorders() {
        let mut tasks = Vec::new();
        upsert(&mut tasks, "a", "working", "scanning");
        upsert(&mut tasks, "b", "working", "editing");
        assert_eq!(tasks.len(), 2);
        // b needs input → it floats to the top.
        upsert(&mut tasks, "b", "needs input", "");
        assert_eq!(tasks[0].agent_id, "b");
        assert_eq!(tasks[0].state, TaskState::NeedsInput);
        // Headline preserved when the update carries none.
        assert_eq!(tasks[0].headline, "editing");
    }

    #[test]
    fn ordering_sections() {
        let mut tasks = Vec::new();
        upsert(&mut tasks, "done1", "done", "x");
        upsert(&mut tasks, "work1", "working", "y");
        upsert(&mut tasks, "need1", "needs input", "z");
        let states: Vec<_> = tasks.iter().map(|t| t.state).collect();
        assert_eq!(
            states,
            vec![TaskState::NeedsInput, TaskState::Working, TaskState::Done]
        );
    }
}
