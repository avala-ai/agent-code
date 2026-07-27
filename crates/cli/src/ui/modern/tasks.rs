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

/// Where a row came from, which decides the group it renders under.
///
/// Subagent rows arrive live from engine events; background rows are
/// polled from the shared `TaskManager`. They are different sources, but
/// the user thinks of them as one list of "things running for me", so
/// they share a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSource {
    /// An Agent-tool subagent, reported via `EngineEvent::SubagentUpdate`.
    Subagent,
    /// A `TaskManager` job: `&`-prefixed shell, workflow, or monitor.
    Background,
}

impl TaskSource {
    pub fn heading(self) -> &'static str {
        match self {
            TaskSource::Subagent => "agents",
            TaskSource::Background => "background",
        }
    }
}

/// One tracked subagent or background task.
#[derive(Debug, Clone)]
pub struct TaskEntry {
    pub agent_id: String,
    pub state: TaskState,
    pub headline: String,
    pub source: TaskSource,
    /// `TaskManager` id backing this row, when one exists — background
    /// rows always, agent rows only for background (`run_in_background`)
    /// runs. Drill-in reads output by this id; `None` means the row is
    /// purely event-driven and has no output file to open.
    pub task_id: Option<String>,
    /// Captured results for a row with no `task_id` — inline subagents,
    /// whose results never reach the `TaskManager` and so have no file to
    /// read. Appended as each one finishes.
    ///
    /// A list rather than one body because `agent_id` is a display id
    /// derived from the call's description: two calls whose descriptions
    /// share a 32-character prefix land on the same row, and keying by it
    /// would let the second result overwrite the first.
    pub outputs: Vec<CapturedOutput>,
}

/// One inline subagent result, tagged with the tool call that produced
/// it so results sharing a row stay distinguishable.
#[derive(Debug, Clone)]
pub struct CapturedOutput {
    pub call_id: String,
    pub body: String,
}

/// One record from the `TaskManager` poll, before reconciliation.
#[derive(Debug, Clone)]
pub struct ManagerRow {
    pub id: String,
    pub state: String,
    pub headline: String,
    /// `Some` for `LocalAgent` runs: the id the subagent's stream
    /// events keyed their row by, so the record folds into that row
    /// instead of listing the same agent twice.
    pub subagent_id: Option<String>,
}

/// Pane lines the grouped list needs: a heading per source group, a
/// blank line between groups, and two lines per task. The layout uses
/// this to size the below-transcript strip on narrow terminals.
pub fn pane_rows(tasks: &[TaskEntry]) -> usize {
    let mut rows = 0;
    let mut last: Option<TaskSource> = None;
    for t in tasks {
        if last != Some(t.source) {
            if last.is_some() {
                rows += 1;
            }
            rows += 1;
            last = Some(t.source);
        }
        rows += 2;
    }
    rows
}

/// Upsert a subagent update into `tasks`, keeping entries ordered by state
/// (needs-input → working → done/failed) with a stable within-section order.
pub fn upsert(tasks: &mut Vec<TaskEntry>, agent_id: &str, state: &str, headline: &str) {
    upsert_with_source(tasks, agent_id, state, headline, TaskSource::Subagent);
}

/// How many inline results one row keeps. Rows collect a result per
/// colliding call, so the list is capped to keep a long session from
/// accumulating bodies without limit; the oldest is dropped first.
const MAX_CAPTURED_PER_ROW: usize = 8;

/// Record a finished inline subagent's output on its row, keyed by the
/// tool call that produced it.
///
/// Inline subagents never reach the `TaskManager`, so they have no
/// `task_id` and nothing on disk to open. Without this the pane shows a
/// finished agent whose result cannot be read anywhere.
///
/// Keyed by `call_id`, not by the row's `agent_id`: that id comes from
/// the call's description, so two calls sharing a description prefix
/// share a row and keying by it would drop one of their results.
/// Re-delivery of the same call replaces its entry in place.
pub fn set_output(tasks: &mut [TaskEntry], agent_id: &str, call_id: &str, output: &str) -> bool {
    let Some(row) = tasks
        .iter_mut()
        .find(|t| t.agent_id == agent_id && t.source == TaskSource::Subagent)
    else {
        return false;
    };
    match row.outputs.iter_mut().find(|o| o.call_id == call_id) {
        Some(existing) => existing.body = output.to_string(),
        None => {
            row.outputs.push(CapturedOutput {
                call_id: call_id.to_string(),
                body: output.to_string(),
            });
            if row.outputs.len() > MAX_CAPTURED_PER_ROW {
                row.outputs.remove(0);
            }
        }
    }
    true
}

/// Upsert a row from a specific source. Background rows are replaced
/// wholesale on each poll, so their state always reflects the manager.
pub fn upsert_with_source(
    tasks: &mut Vec<TaskEntry>,
    agent_id: &str,
    state: &str,
    headline: &str,
    source: TaskSource,
) {
    let state = TaskState::parse(state);
    // Match within the source: an agent whose id happens to equal a
    // background task id (e.g. a description of "b1") must not update
    // that unrelated row — the next poll would rebuild it and drop the
    // agent from the pane entirely.
    if let Some(existing) = tasks
        .iter_mut()
        .find(|t| t.agent_id == agent_id && t.source == source)
    {
        existing.state = state;
        if !headline.is_empty() {
            existing.headline = headline.to_string();
        }
    } else {
        tasks.push(TaskEntry {
            agent_id: agent_id.to_string(),
            state,
            headline: headline.to_string(),
            source,
            task_id: None,
            outputs: Vec::new(),
        });
    }
    // Group by source, then float needs-input rows to the top within it.
    // Stable, so arrival order is preserved inside a section.
    tasks.sort_by_key(|t| (t.source.heading(), t.state.order()));
}

#[cfg(test)]
mod tests {
    /// The drill-in dead end this closes: an inline subagent finishes,
    /// has no `task_id` because it never reached the TaskManager, and so
    /// there is nothing to open. Its result is captured on the row.
    #[test]
    fn a_finished_inline_subagent_keeps_its_output() {
        let mut tasks = Vec::new();
        upsert(&mut tasks, "explorer", "working", "look around");
        assert!(tasks[0].outputs.is_empty());
        assert!(tasks[0].task_id.is_none(), "inline rows have no task id");

        assert!(set_output(
            &mut tasks,
            "explorer",
            "c1",
            "found three things"
        ));
        assert_eq!(tasks[0].outputs.len(), 1);
        assert_eq!(tasks[0].outputs[0].body, "found three things");
    }

    /// Background rows read their output from the TaskManager by id, so
    /// captured output must not be attached to them — it would shadow
    /// the live file.
    #[test]
    fn output_is_not_attached_to_background_rows() {
        let mut tasks = Vec::new();
        upsert_with_source(&mut tasks, "b1", "done", "build", TaskSource::Background);
        assert!(
            !set_output(&mut tasks, "b1", "c1", "stale"),
            "captured output was attached to a manager-backed row"
        );
        assert!(tasks[0].outputs.is_empty());
    }

    #[test]
    fn output_for_an_unknown_agent_is_ignored() {
        let mut tasks = Vec::new();
        upsert(&mut tasks, "explorer", "working", "look");
        assert!(!set_output(&mut tasks, "someone-else", "c1", "hi"));
        assert!(tasks[0].outputs.is_empty());
    }

    /// A later status update must not wipe the captured result — the
    /// upsert path rewrites state and headline, and output has to
    /// survive that.
    #[test]
    fn a_later_update_does_not_drop_captured_output() {
        let mut tasks = Vec::new();
        upsert(&mut tasks, "explorer", "working", "look");
        set_output(&mut tasks, "explorer", "c1", "the answer");
        upsert(&mut tasks, "explorer", "done", "finished");
        assert_eq!(
            tasks[0].outputs[0].body, "the answer",
            "a status update discarded the captured output"
        );
    }

    /// The collision this guards: `agent_id` is derived from the call's
    /// description, so two Agent calls whose descriptions share a
    /// 32-character prefix land on one row. Keyed by that id, the second
    /// result overwrote the first and one finished agent had nothing to
    /// open. Keyed by the tool call, both survive.
    #[test]
    fn two_calls_sharing_a_row_each_keep_their_result() {
        let mut tasks = Vec::new();
        let shared = "audit the authentication module for"; // > 32 chars
        upsert(&mut tasks, shared, "working", "audit A");
        set_output(&mut tasks, shared, "call-a", "A found a bug");
        set_output(&mut tasks, shared, "call-b", "B found nothing");

        assert_eq!(tasks.len(), 1, "the rows collapse — that is the premise");
        let bodies: Vec<&str> = tasks[0].outputs.iter().map(|o| o.body.as_str()).collect();
        assert_eq!(
            bodies,
            vec!["A found a bug", "B found nothing"],
            "one call's result overwrote the other"
        );
    }

    /// Re-delivery of the same call replaces its entry rather than
    /// stacking duplicates.
    #[test]
    fn the_same_call_reported_twice_replaces_its_entry() {
        let mut tasks = Vec::new();
        upsert(&mut tasks, "explorer", "done", "look");
        set_output(&mut tasks, "explorer", "c1", "first");
        set_output(&mut tasks, "explorer", "c1", "corrected");
        assert_eq!(tasks[0].outputs.len(), 1);
        assert_eq!(tasks[0].outputs[0].body, "corrected");
    }

    /// Captured bodies are bounded per row, so a long session that keeps
    /// hitting one row cannot grow it without limit.
    #[test]
    fn captured_outputs_are_capped_per_row() {
        let mut tasks = Vec::new();
        upsert(&mut tasks, "explorer", "done", "look");
        for i in 0..MAX_CAPTURED_PER_ROW + 3 {
            set_output(
                &mut tasks,
                "explorer",
                &format!("c{i}"),
                &format!("body {i}"),
            );
        }
        assert_eq!(tasks[0].outputs.len(), MAX_CAPTURED_PER_ROW);
        assert_eq!(
            tasks[0].outputs.last().unwrap().body,
            format!("body {}", MAX_CAPTURED_PER_ROW + 2),
            "the newest result must be kept"
        );
        assert_eq!(
            tasks[0].outputs[0].call_id, "c3",
            "the oldest results are the ones dropped"
        );
    }

    /// The pane was fed only by `EngineEvent::SubagentUpdate`, so a
    /// background job (`&` shell, workflow, monitor) never appeared in it
    /// at all — the user had to run `/tasks list` to see one.
    #[test]
    fn background_rows_and_subagent_rows_share_the_pane() {
        let mut tasks = Vec::new();
        upsert(&mut tasks, "agent-1", "working", "explore parser");
        upsert_with_source(
            &mut tasks,
            "b3",
            "working",
            "cargo build --release",
            TaskSource::Background,
        );
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|t| t.source == TaskSource::Subagent));
        assert!(tasks.iter().any(|t| t.source == TaskSource::Background));
    }

    /// Rows group by source so the pane reads as two lists, and
    /// needs-input still floats to the top within its group.
    #[test]
    fn rows_group_by_source_then_by_state() {
        let mut tasks = Vec::new();
        upsert_with_source(&mut tasks, "b1", "done", "build", TaskSource::Background);
        upsert(&mut tasks, "a1", "working", "explore");
        upsert_with_source(&mut tasks, "b2", "working", "test", TaskSource::Background);
        upsert(&mut tasks, "a2", "needs_input", "asks");

        let sources: Vec<_> = tasks.iter().map(|t| t.source).collect();
        // All of one source before all of the other — no interleaving.
        let first = sources[0];
        let split = sources
            .iter()
            .position(|s| *s != first)
            .unwrap_or(sources.len());
        assert!(
            sources[split..].iter().all(|s| *s != first),
            "sources interleaved: {sources:?}"
        );

        let agents: Vec<_> = tasks
            .iter()
            .filter(|t| t.source == TaskSource::Subagent)
            .collect();
        assert_eq!(
            agents[0].state,
            TaskState::NeedsInput,
            "needs-input did not float to the top of its group"
        );
    }

    use super::*;

    /// An agent id colliding with a background task id must create its
    /// own row in its own group, not hijack the background row.
    #[test]
    fn colliding_ids_across_sources_stay_separate_rows() {
        let mut tasks = Vec::new();
        upsert_with_source(&mut tasks, "b1", "working", "build", TaskSource::Background);
        upsert(&mut tasks, "b1", "working", "agent named b1");
        assert_eq!(tasks.len(), 2, "sources shared a row");
        assert!(
            tasks
                .iter()
                .any(|t| t.source == TaskSource::Subagent && t.headline == "agent named b1")
        );
        assert!(
            tasks
                .iter()
                .any(|t| t.source == TaskSource::Background && t.headline == "build")
        );
    }

    /// Heading + gap accounting for the layout's strip sizing: agents
    /// heading, two rows, blank, background heading, two rows = 7.
    #[test]
    fn pane_rows_counts_headings_gaps_and_task_lines() {
        assert_eq!(pane_rows(&[]), 0);
        let mut tasks = Vec::new();
        upsert(&mut tasks, "a1", "working", "explore");
        assert_eq!(pane_rows(&tasks), 3);
        upsert_with_source(&mut tasks, "b1", "working", "build", TaskSource::Background);
        assert_eq!(pane_rows(&tasks), 7);
    }

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
