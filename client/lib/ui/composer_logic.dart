/// Pure logic behind the composer: slash-command matching, pasted-text
/// handling, and draft insertion. Kept out of the widget so each rule can be
/// tested on its own.
///
/// Ported from qm's `plugins/web-ui/src/composer.ts` and `paste-text.ts`.
library;

/// A user-invocable skill, as offered by the slash picker.
class SkillItem {
  final String name;
  final String? description;
  final String? argumentHint;

  const SkillItem({required this.name, this.description, this.argumentHint});

  factory SkillItem.fromJson(Map<String, dynamic> json) => SkillItem(
        name: json['name'] as String? ?? '',
        description: json['description'] as String?,
        argumentHint: json['argument_hint'] as String?,
      );
}

/// A skill matched against the current query, carrying the span of the match so
/// the picker can embolden the typed letters.
class SkillMatch {
  final SkillItem skill;

  /// Index of the match within the skill name, or -1 when the query is empty
  /// and every skill is offered unfiltered.
  final int start;
  final int end;

  const SkillMatch({required this.skill, required this.start, required this.end});

  bool get hasSpan => start >= 0;
}

/// Matches a trailing `/word` at a word boundary — the token the picker filters
/// on. Anchored to the end so the menu only opens while the caret is on it.
final RegExp slashToken = RegExp(r'(^|\s)/([a-zA-Z0-9_-]*)$');

/// The active slash query in [draft], or null when the caret is not on a slash
/// token. An empty string means `/` was just typed and everything matches.
String? slashQuery(String draft) {
  final match = slashToken.firstMatch(draft);
  return match?.group(2);
}

/// Skills matching [query], ordered by where the match falls (a prefix match
/// beats a mid-name one) and then by name.
List<SkillMatch> matchSkills(String query, List<SkillItem> skills) {
  final q = query.toLowerCase();
  if (q.isEmpty) {
    return [
      for (final skill in skills) SkillMatch(skill: skill, start: -1, end: -1),
    ];
  }

  final out = <SkillMatch>[];
  for (final skill in skills) {
    final at = skill.name.toLowerCase().indexOf(q);
    if (at >= 0) {
      out.add(SkillMatch(skill: skill, start: at, end: at + q.length));
    }
  }
  out.sort((a, b) {
    final byPosition = a.start.compareTo(b.start);
    return byPosition != 0 ? byPosition : a.skill.name.compareTo(b.skill.name);
  });
  return out;
}

/// Replaces the active slash token in [draft] with the chosen skill, leaving a
/// trailing space so arguments can be typed straight away.
String acceptSkill(String draft, SkillItem skill) =>
    draft.replaceFirstMapped(slashToken, (m) => '${m.group(1)}/${skill.name} ');

/// Pastes above this many characters become an attachment chip rather than
/// being dumped into the input, where they would bury the actual prompt.
const int kPasteChipThreshold = 800;

bool shouldChipPaste(String text) => text.length >= kPasteChipThreshold;

/// The label for a pasted-text chip: `Pasted text · 12.4k chars`.
String pasteChipLabel(int charCount) {
  final k = charCount / 1000;
  final String count;
  if (charCount < 1000) {
    count = '$charCount';
  } else if (k < 9.95) {
    count = '${k.toStringAsFixed(1)}k';
  } else {
    count = '${k.round()}k';
  }
  return 'Pasted text · $count chars';
}

/// The result of inserting text into a draft: the new text and where the caret
/// should land.
class DraftInsertion {
  final String draft;
  final int cursor;

  const DraftInsertion(this.draft, this.cursor);
}

/// Inserts [text] into [draft] at [cursor], padding with newlines so it lands
/// on its own line rather than running into surrounding prose.
DraftInsertion insertIntoDraft(String draft, String text, int? cursor) {
  final at = (cursor == null || cursor < 0 || cursor > draft.length)
      ? draft.length
      : cursor;
  final before = draft.substring(0, at);
  final after = draft.substring(at);
  final lead = before.isNotEmpty && !before.endsWith('\n') ? '\n' : '';
  final trail = after.isNotEmpty && !after.startsWith('\n') ? '\n' : '';
  return DraftInsertion(
    '$before$lead$text$trail$after',
    before.length + lead.length + text.length,
  );
}

/// The composer's placeholder for the current turn state.
///
/// A running turn does not lock the input — it invites a steer. Only an
/// outstanding approval does, because the turn genuinely cannot advance.
String composerPlaceholder({
  required bool inputBlocked,
  required bool streaming,
}) {
  if (inputBlocked) return 'Approve or deny to continue';
  if (streaming) return 'Steer the running task…';
  return 'Ask anything';
}
