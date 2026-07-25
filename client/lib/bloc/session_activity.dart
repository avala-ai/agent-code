/// Live activity state for a single agent session, surfaced as a badge in the
/// session sidebar so the list reads as a dashboard.
///
/// The ordering and wording mirror the terminal client's `TaskState`
/// (`crates/cli/src/ui/modern/tasks.rs`) so the two surfaces agree, with an
/// extra [idle] state for a connected-but-quiet session.
enum SessionActivity {
  idle,
  working,
  needsInput,
  done,
  failed;

  /// Human-readable label for the badge.
  String get label => switch (this) {
        SessionActivity.idle => 'Idle',
        SessionActivity.working => 'Working',
        SessionActivity.needsInput => 'Needs input',
        SessionActivity.done => 'Done',
        SessionActivity.failed => 'Failed',
      };

  /// Status glyph for compact rendering.
  String get glyph => switch (this) {
        SessionActivity.working => '●',
        SessionActivity.needsInput => '◐',
        SessionActivity.idle => '○',
        SessionActivity.done => '✓',
        SessionActivity.failed => '✗',
      };

  /// Section sort key: needs-input first, then working, then idle, then
  /// finished. Matches the terminal tasks pane (needs-input → working → …).
  int get order => switch (this) {
        SessionActivity.needsInput => 0,
        SessionActivity.working => 1,
        SessionActivity.idle => 2,
        SessionActivity.done => 3,
        SessionActivity.failed => 4,
      };
}
