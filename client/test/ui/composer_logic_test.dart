import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_client_app/ui/composer_logic.dart';

const _skills = [
  SkillItem(name: 'commit', description: 'Write a commit'),
  SkillItem(name: 'review', description: 'Review the diff'),
  SkillItem(name: 'plan', description: 'Plan the work'),
  SkillItem(name: 'code-review', description: 'Deep review'),
];

void main() {
  group('slashQuery', () {
    test('a bare slash opens the picker with an empty query', () {
      expect(slashQuery('/'), '');
    });

    test('captures the typed letters', () {
      expect(slashQuery('/com'), 'com');
    });

    test('opens after a space mid-draft', () {
      expect(slashQuery('please run /rev'), 'rev');
    });

    test('does not open mid-word', () {
      expect(slashQuery('http://exa'), isNull);
      expect(slashQuery('and/or'), isNull);
    });

    test('does not open when the caret has moved past the token', () {
      expect(slashQuery('/commit '), isNull);
      expect(slashQuery('/commit and then'), isNull);
    });

    test('is null for ordinary prose', () {
      expect(slashQuery('just a message'), isNull);
      expect(slashQuery(''), isNull);
    });

    test('accepts hyphens and underscores', () {
      expect(slashQuery('/code-rev'), 'code-rev');
      expect(slashQuery('/my_skill'), 'my_skill');
    });

    test('opens on a newline boundary', () {
      expect(slashQuery('first line\n/com'), 'com');
    });
  });

  group('matchSkills', () {
    test('an empty query offers everything unfiltered', () {
      final matches = matchSkills('', _skills);
      expect(matches, hasLength(4));
      expect(matches.every((m) => !m.hasSpan), isTrue);
    });

    test('filters by substring', () {
      final matches = matchSkills('rev', _skills);
      expect(matches.map((m) => m.skill.name), ['review', 'code-review']);
    });

    test('prefix matches sort ahead of mid-name matches', () {
      final matches = matchSkills('rev', _skills);
      expect(matches.first.skill.name, 'review');
      expect(matches.first.start, 0);
      expect(matches.last.start, 5);
    });

    test('reports the span of the match for emboldening', () {
      final match = matchSkills('mit', _skills).single;
      expect(match.skill.name, 'commit');
      expect(match.start, 3);
      expect(match.end, 6);
    });

    test('is case-insensitive', () {
      expect(matchSkills('COM', _skills).single.skill.name, 'commit');
    });

    test('ties break alphabetically', () {
      const skills = [
        SkillItem(name: 'zebra'),
        SkillItem(name: 'apple'),
      ];
      final matches = matchSkills('e', skills);
      expect(matches.map((m) => m.skill.name), ['zebra', 'apple'],
          reason: 'zebra matches at 2, apple at 4 — position wins over name');
    });

    test('no match yields an empty list', () {
      expect(matchSkills('zzz', _skills), isEmpty);
    });
  });

  group('acceptSkill', () {
    test('replaces the token and leaves a trailing space', () {
      expect(acceptSkill('/com', _skills[0]), '/commit ');
    });

    test('preserves the text before the token', () {
      expect(acceptSkill('please run /rev', _skills[1]), 'please run /review ');
    });

    test('works from a bare slash', () {
      expect(acceptSkill('/', _skills[2]), '/plan ');
    });

    test('preserves a newline boundary', () {
      expect(acceptSkill('line one\n/pl', _skills[2]), 'line one\n/plan ');
    });
  });

  group('pasteChipLabel', () {
    test('counts small pastes exactly', () {
      expect(pasteChipLabel(42), 'Pasted text · 42 chars');
      expect(pasteChipLabel(999), 'Pasted text · 999 chars');
    });

    test('uses one decimal below ten thousand', () {
      expect(pasteChipLabel(1000), 'Pasted text · 1.0k chars');
      expect(pasteChipLabel(2450), 'Pasted text · 2.5k chars');
      expect(pasteChipLabel(9940), 'Pasted text · 9.9k chars');
    });

    test('rounds to whole thousands above that', () {
      expect(pasteChipLabel(9950), 'Pasted text · 10k chars');
      expect(pasteChipLabel(12400), 'Pasted text · 12k chars');
      expect(pasteChipLabel(120000), 'Pasted text · 120k chars');
    });
  });

  group('shouldChipPaste', () {
    test('short pastes go straight into the draft', () {
      expect(shouldChipPaste('a short snippet'), isFalse);
    });

    test('long pastes become a chip', () {
      expect(shouldChipPaste('x' * 2000), isTrue);
    });

    test('the threshold is inclusive', () {
      expect(shouldChipPaste('x' * kPasteChipThreshold), isTrue);
      expect(shouldChipPaste('x' * (kPasteChipThreshold - 1)), isFalse);
    });
  });

  group('insertIntoDraft', () {
    test('inserting into an empty draft adds no padding', () {
      final r = insertIntoDraft('', 'text', 0);
      expect(r.draft, 'text');
      expect(r.cursor, 4);
    });

    test('appends on its own line', () {
      final r = insertIntoDraft('prompt', 'text', 6);
      expect(r.draft, 'prompt\ntext');
      expect(r.cursor, 11);
    });

    test('inserting mid-draft pads both sides', () {
      final r = insertIntoDraft('beforeafter', 'X', 6);
      expect(r.draft, 'before\nX\nafter');
      expect(r.cursor, 8);
    });

    test('does not double up an existing newline', () {
      final r = insertIntoDraft('prompt\n', 'text', 7);
      expect(r.draft, 'prompt\ntext');
    });

    test('a null cursor appends at the end', () {
      final r = insertIntoDraft('prompt', 'text', null);
      expect(r.draft, 'prompt\ntext');
    });

    test('an out-of-range cursor appends at the end', () {
      expect(insertIntoDraft('abc', 'X', 99).draft, 'abc\nX');
      expect(insertIntoDraft('abc', 'X', -5).draft, 'abc\nX');
    });
  });

  group('composerPlaceholder', () {
    test('idle invites a prompt', () {
      expect(
        composerPlaceholder(inputBlocked: false, streaming: false),
        'Ask anything',
      );
    });

    test('a running turn invites a steer rather than locking', () {
      expect(
        composerPlaceholder(inputBlocked: false, streaming: true),
        'Steer the running task…',
      );
    });

    test('a pending approval takes precedence over streaming', () {
      expect(
        composerPlaceholder(inputBlocked: true, streaming: true),
        'Approve or deny to continue',
      );
    });
  });
}
