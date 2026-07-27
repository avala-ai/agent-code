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
/// Rows the pane needs for the checklist and the grouped task list,
/// with `collapsed` groups folded to their heading.
pub fn pane_rows_with_todos(
    tasks: &[TaskEntry],
    todos: &[TodoItem],
    collapsed: &[TaskSource],
) -> usize {
    let mut rows = pane_rows_collapsed(tasks, collapsed);
    if !todos.is_empty() {
        // heading + one row per item, plus a blank separator when tasks
        // follow it.
        rows += 1 + todos.len();
        if !tasks.is_empty() {
            rows += 1;
        }
    }
    rows
}

pub fn pane_rows(tasks: &[TaskEntry]) -> usize {
    pane_rows_collapsed(tasks, &[])
}

/// Row count with `collapsed` groups folded to their heading.
///
/// The pane is sized from this, so a collapsed group has to shrink the
/// strip too — otherwise collapsing frees no space and buys nothing.
pub fn pane_rows_collapsed(tasks: &[TaskEntry], collapsed: &[TaskSource]) -> usize {
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
        if !collapsed.contains(&t.source) {
            rows += 2;
        }
    }
    rows
}

/// Rows the user can move a selection through.
///
/// An expanded group contributes every one of its rows. A collapsed
/// group contributes exactly one: its first task, which is the index
/// the folded heading stands in for. Dropping it instead would make the
/// group the user just folded the one group they can no longer reach to
/// unfold.
pub fn selectable_indices(tasks: &[TaskEntry], collapsed: &[TaskSource]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut last: Option<TaskSource> = None;
    for (i, t) in tasks.iter().enumerate() {
        if !collapsed.contains(&t.source) || last != Some(t.source) {
            out.push(i);
        }
        last = Some(t.source);
    }
    out
}

/// True when `idx` is the row a collapsed group's heading stands in for.
///
/// Rows are sorted by source, so a group is one contiguous run and its
/// first member is the only one the heading can represent.
pub fn is_folded_heading(tasks: &[TaskEntry], collapsed: &[TaskSource], idx: usize) -> bool {
    let Some(t) = tasks.get(idx) else {
        return false;
    };
    collapsed.contains(&t.source) && (idx == 0 || tasks[idx - 1].source != t.source)
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
    // The crate root allows dead code for its public API surface,
    // which also silences a test that loses its `#[test]`. Opt back in:
    // an unannotated test is unreachable, so the compiler should be the
    // thing that notices.
    #![deny(dead_code)]

    fn two_groups() -> Vec<TaskEntry> {
        let mut t = Vec::new();
        upsert(&mut t, "a1", "working", "explore");
        upsert(&mut t, "a2", "working", "audit");
        upsert_with_source(&mut t, "b1", "working", "build", TaskSource::Background);
        t
    }

    /// Collapsing has to shrink the pane, or it frees no space and buys
    /// the user nothing.
    #[test]
    fn a_collapsed_group_costs_only_its_heading() {
        let tasks = two_groups();
        let open = pane_rows_collapsed(&tasks, &[]);
        let folded = pane_rows_collapsed(&tasks, &[TaskSource::Subagent]);
        assert!(
            folded < open,
            "collapsing did not shrink the pane: {folded} vs {open}"
        );
        // Two agent rows at 2 lines each disappear; the heading stays.
        assert_eq!(open - folded, 4);
    }

    /// Sizing has to see both the checklist and the folded groups: the
    /// two features landed separately, and a row count that knows about
    /// only one of them either overshoots the strip or gives collapsing
    /// nothing to free.
    #[test]
    fn collapsing_still_shrinks_the_pane_when_a_checklist_is_present() {
        let tasks = two_groups();
        let todos = vec![
            TodoItem {
                id: "1".into(),
                content: "first".into(),
                status: TodoStatus::Done,
            },
            TodoItem {
                id: "2".into(),
                content: "second".into(),
                status: TodoStatus::InProgress,
            },
        ];
        let open = pane_rows_with_todos(&tasks, &todos, &[]);
        let folded = pane_rows_with_todos(&tasks, &todos, &[TaskSource::Subagent]);
        // The checklist costs the same either way, so folding frees
        // exactly the two agent rows it hides.
        assert_eq!(open - folded, 4, "{open} vs {folded}");
        // And the checklist is still accounted for on top of the tasks.
        assert!(
            open > pane_rows_collapsed(&tasks, &[]),
            "checklist rows vanished from the row count"
        );
    }

    #[test]
    fn pane_rows_matches_the_uncollapsed_count() {
        let tasks = two_groups();
        assert_eq!(pane_rows(&tasks), pane_rows_collapsed(&tasks, &[]));
    }

    /// A folded group keeps exactly one selectable row — its heading —
    /// so the user can still reach it to unfold. The rows behind the
    /// heading stay unreachable: landing there would look like the pane
    /// stopped responding.
    #[test]
    fn a_folded_group_is_selectable_only_through_its_heading() {
        let tasks = two_groups();
        let sel = selectable_indices(&tasks, &[TaskSource::Subagent]);
        let agents: Vec<usize> = sel
            .iter()
            .copied()
            .filter(|i| tasks[*i].source == TaskSource::Subagent)
            .collect();
        assert_eq!(
            agents.len(),
            1,
            "folded group must contribute exactly one selectable row"
        );
        assert!(
            is_folded_heading(&tasks, &[TaskSource::Subagent], agents[0]),
            "the selectable row is not the group's heading"
        );
        // The other group is untouched.
        assert_eq!(
            sel.iter()
                .filter(|i| tasks[**i].source == TaskSource::Background)
                .count(),
            1
        );
        assert_eq!(selectable_indices(&tasks, &[]).len(), tasks.len());
    }

    /// Folding every group must not strand the selection with nowhere
    /// to go — each heading stays reachable.
    #[test]
    fn folding_everything_leaves_one_row_per_group() {
        let tasks = two_groups();
        let sel = selectable_indices(&tasks, &[TaskSource::Subagent, TaskSource::Background]);
        assert_eq!(sel.len(), 2, "expected one heading per group: {sel:?}");
        assert!(
            sel.iter().all(|i| is_folded_heading(
                &tasks,
                &[TaskSource::Subagent, TaskSource::Background],
                *i
            )),
            "a non-heading row stayed selectable"
        );
    }

    #[test]
    fn only_a_groups_first_row_counts_as_its_heading() {
        let tasks = two_groups();
        let folded = [TaskSource::Subagent];
        let agent_rows: Vec<usize> = tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.source == TaskSource::Subagent)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(agent_rows.len(), 2);
        assert!(is_folded_heading(&tasks, &folded, agent_rows[0]));
        assert!(!is_folded_heading(&tasks, &folded, agent_rows[1]));
        // An expanded group has no folded heading at all.
        assert!(!is_folded_heading(&tasks, &[], agent_rows[0]));
    }

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

/// A checklist entry from the model's latest `TodoWrite` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Done,
}

impl TodoItem {
    pub fn from_fields((id, content, status): super::sink::TodoFields) -> Self {
        TodoItem {
            id,
            content,
            status: TodoStatus::parse(&status),
        }
    }
}

/// The entries of a `TodoWrite` input, or `None` when any entry is
/// missing one of the three fields the tool declares required.
///
/// Shared by the live sink and the session-restore path so a checklist
/// is held to the same shape however it reaches the pane. One bad entry
/// rejects the batch: a partial checklist misdescribes the plan as badly
/// as an empty one.
pub fn parse_todo_input(input: &serde_json::Value) -> Option<Vec<super::sink::TodoFields>> {
    input
        .get("todos")?
        .as_array()?
        .iter()
        .map(|t| {
            let field = |k: &str| t.get(k)?.as_str().map(str::to_string);
            Some((field("id")?, field("content")?, field("status")?))
        })
        .collect()
}

/// The checklist a stored conversation ended with: the input of its last
/// `TodoWrite` that came back without an error.
///
/// A command can swap or rewrite the conversation out from under the UI
/// (`/resume`, the session picker, `/rewind`, `/snip`). The pane is
/// cached state rather than a view of the messages, so without this it
/// would go on describing a plan the history no longer contains.
///
/// A call counts only with a matching result that is *not* an error —
/// success must be positively evidenced, not inferred from the absence
/// of a failure. A `TodoWrite` with no result at all never ran (a turn
/// cut short by max-token recovery records the assistant message before
/// tool execution, and that history can be saved and resumed), and the
/// live sink withholds exactly those. Treating them as successful here
/// would let a restored pane show a checklist the streamed pane refused.
pub fn todos_from_messages(messages: &[agent_code_lib::llm::message::Message]) -> Vec<TodoItem> {
    use agent_code_lib::llm::message::{ContentBlock, Message};

    fn blocks(m: &Message) -> &[ContentBlock] {
        match m {
            Message::User(u) => &u.content,
            Message::Assistant(a) => &a.content,
            Message::System(_) => &[],
        }
    }
    let succeeded: std::collections::HashSet<&str> = messages
        .iter()
        .flat_map(blocks)
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error: false,
                ..
            } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();

    messages
        .iter()
        .rev()
        .flat_map(|m| blocks(m).iter().rev())
        .find_map(|b| match b {
            ContentBlock::ToolUse { id, name, input }
                if name == "TodoWrite" && succeeded.contains(id.as_str()) =>
            {
                parse_todo_input(input)
            }
            _ => None,
        })
        .map(|fields| fields.into_iter().map(TodoItem::from_fields).collect())
        .unwrap_or_default()
}

impl TodoStatus {
    /// Map the tool's status string. Unknown values read as pending,
    /// which is the honest default: an item nobody claimed is not done.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "done" | "completed" | "complete" => TodoStatus::Done,
            "in_progress" | "in progress" | "active" => TodoStatus::InProgress,
            _ => TodoStatus::Pending,
        }
    }

    /// Checklist glyph, matching the markers `TodoWrite` already prints.
    pub fn glyph(self) -> &'static str {
        match self {
            TodoStatus::Done => "✔",
            TodoStatus::InProgress => "◐",
            TodoStatus::Pending => "□",
        }
    }
}

/// The slice of checklist items the pane can draw in `budget` rows, as
/// `(start, len)`.
///
/// A checklist is model-authored and unbounded — nothing stops a plan
/// with two hundred entries — so the pane windows it instead of letting
/// it claim every row. The window is anchored on the item in flight
/// (falling back to the first unfinished one), because that is the row
/// the user is actually tracking, with a little lead-in above it for
/// context.
pub fn todo_window(todos: &[TodoItem], budget: usize) -> (usize, usize) {
    if todos.len() <= budget {
        return (0, todos.len());
    }
    if budget == 0 {
        return (0, 0);
    }
    let anchor = todos
        .iter()
        .position(|t| t.status == TodoStatus::InProgress)
        .or_else(|| todos.iter().position(|t| t.status != TodoStatus::Done))
        .unwrap_or(0);
    let lead = budget / 3;
    let start = anchor.saturating_sub(lead).min(todos.len() - budget);
    (start, budget)
}

/// `2/5` — how far through the checklist the model is.
pub fn todo_progress(todos: &[TodoItem]) -> (usize, usize) {
    let done = todos
        .iter()
        .filter(|t| t.status == TodoStatus::Done)
        .count();
    (done, todos.len())
}

#[cfg(test)]
mod todo_tests {
    #![deny(dead_code)]

    use super::*;

    #[test]
    fn status_parsing_defaults_to_pending() {
        assert_eq!(TodoStatus::parse("done"), TodoStatus::Done);
        assert_eq!(TodoStatus::parse("completed"), TodoStatus::Done);
        assert_eq!(TodoStatus::parse("in_progress"), TodoStatus::InProgress);
        assert_eq!(TodoStatus::parse("pending"), TodoStatus::Pending);
        // An unrecognised status must not read as finished.
        assert_eq!(TodoStatus::parse("garbage"), TodoStatus::Pending);
        assert_eq!(TodoStatus::parse(""), TodoStatus::Pending);
    }

    fn todo(content: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            id: content.into(),
            content: content.into(),
            status,
        }
    }

    /// A long checklist must not claim more rows than it is given, and
    /// the window has to keep the item being worked on — the only one
    /// the user is really tracking — inside it.
    #[test]
    fn the_checklist_window_keeps_the_item_in_flight_visible() {
        let mut todos: Vec<TodoItem> = (0..40)
            .map(|i| todo(&format!("item {i}"), TodoStatus::Done))
            .collect();
        todos[30].status = TodoStatus::InProgress;
        for t in todos.iter_mut().skip(31) {
            t.status = TodoStatus::Pending;
        }

        let (start, len) = todo_window(&todos, 6);
        assert_eq!(len, 6, "window exceeded its budget");
        assert!(
            (start..start + len).contains(&30),
            "in-progress item {} fell outside window {start}..{}",
            30,
            start + len
        );

        // With no in-progress item, the first unfinished one anchors it.
        let mut all_but_last_done: Vec<TodoItem> = (0..40)
            .map(|i| todo(&format!("item {i}"), TodoStatus::Done))
            .collect();
        all_but_last_done[39].status = TodoStatus::Pending;
        let (start, len) = todo_window(&all_but_last_done, 5);
        assert!(
            (start..start + len).contains(&39),
            "the only unfinished item was windowed out"
        );

        // Short lists are shown whole; a zero budget shows nothing rather
        // than panicking on the slice.
        assert_eq!(todo_window(&todos[..3], 6), (0, 3));
        assert_eq!(todo_window(&todos, 0), (0, 0));
        assert_eq!(todo_window(&[], 4), (0, 0));
    }

    /// `/resume` swaps the conversation under a cached pane. Rebuilding
    /// from the messages must find the last checklist that actually ran
    /// — the same rule the live path applies — so the restored pane
    /// agrees with what the session ended on.
    #[test]
    fn a_restored_conversation_yields_its_last_successful_checklist() {
        use agent_code_lib::llm::message::{AssistantMessage, ContentBlock, Message};

        let todo_call = |id: &str, content: &str| ContentBlock::ToolUse {
            id: id.into(),
            name: "TodoWrite".into(),
            input: serde_json::json!({
                "todos": [{ "id": "1", "content": content, "status": "in_progress" }]
            }),
        };
        let assistant = |blocks: Vec<ContentBlock>| {
            Message::Assistant(AssistantMessage {
                uuid: uuid::Uuid::new_v4(),
                timestamp: String::new(),
                content: blocks,
                model: None,
                usage: None,
                stop_reason: None,
                request_id: None,
            })
        };
        let result = |id: &str, is_error: bool| ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: "ok".into(),
            is_error,
            extra_content: Vec::new(),
        };

        // Latest successful call wins over an earlier one.
        let msgs = vec![
            assistant(vec![todo_call("t1", "the older plan")]),
            assistant(vec![result("t1", false)]),
            assistant(vec![todo_call("t2", "the newer plan")]),
            assistant(vec![result("t2", false)]),
        ];
        let todos = todos_from_messages(&msgs);
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].content, "the newer plan");
        assert_eq!(todos[0].status, TodoStatus::InProgress);

        // A denied or failed call is skipped in favour of the last one
        // that ran — a rejected plan never described the session.
        let msgs = vec![
            assistant(vec![todo_call("t1", "the plan that ran")]),
            assistant(vec![result("t1", false)]),
            assistant(vec![todo_call("t2", "the denied plan")]),
            assistant(vec![result("t2", true)]),
        ];
        let todos = todos_from_messages(&msgs);
        assert_eq!(todos.len(), 1, "the denied call was published");
        assert_eq!(todos[0].content, "the plan that ran");

        // A call with no result at all never ran — a turn cut short
        // before tool execution records the assistant message anyway,
        // and that history can be saved and resumed. The live sink
        // withholds those, so restoration must too, or the two panes
        // disagree. Success has to be evidenced, not assumed.
        let msgs = vec![
            assistant(vec![todo_call("t1", "the plan that ran")]),
            assistant(vec![result("t1", false)]),
            assistant(vec![todo_call("t2", "the plan that never executed")]),
        ];
        let todos = todos_from_messages(&msgs);
        assert_eq!(todos.len(), 1, "an unresolved call was published");
        assert_eq!(
            todos[0].content, "the plan that ran",
            "an unresolved TodoWrite was treated as successful"
        );

        // The only call being unresolved leaves the pane empty rather
        // than showing a checklist that never took effect.
        assert!(
            todos_from_messages(&[assistant(vec![todo_call("t1", "never ran")])]).is_empty(),
            "a lone unresolved call was published"
        );

        // A session that never wrote a checklist clears the pane rather
        // than leaving the previous session's plan up.
        assert!(todos_from_messages(&[]).is_empty());
        assert!(
            todos_from_messages(&[assistant(vec![ContentBlock::Text {
                text: "no todos here".into()
            }])])
            .is_empty()
        );

        // Malformed entries are held to the same shape as the live path,
        // even when the call itself succeeded.
        let malformed = vec![
            assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "TodoWrite".into(),
                input: serde_json::json!({ "todos": [{ "id": "1" }] }),
            }]),
            assistant(vec![result("t1", false)]),
        ];
        assert!(todos_from_messages(&malformed).is_empty());
    }

    #[test]
    fn progress_counts_only_completed_items() {
        let todos = vec![
            TodoItem {
                id: "1".into(),
                content: "a".into(),
                status: TodoStatus::Done,
            },
            TodoItem {
                id: "2".into(),
                content: "b".into(),
                status: TodoStatus::InProgress,
            },
            TodoItem {
                id: "3".into(),
                content: "c".into(),
                status: TodoStatus::Pending,
            },
        ];
        assert_eq!(todo_progress(&todos), (1, 3));
        assert_eq!(todo_progress(&[]), (0, 0));
    }
}
