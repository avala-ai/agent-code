import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_client_app/ui/app_theme.dart';
import 'package:agent_code_client_app/ui/composer.dart';
import 'package:agent_code_client_app/ui/composer_logic.dart';

const _skills = [
  SkillItem(name: 'commit', description: 'Write a commit'),
  SkillItem(name: 'review', description: 'Review the diff'),
  SkillItem(name: 'code-review', description: 'Deep review'),
];

Widget _host({
  bool streaming = false,
  void Function(String)? onSend,
}) =>
    MaterialApp(
      theme: buildAppTheme(Brightness.light),
      home: Scaffold(
        body: Composer(
          streaming: streaming,
          inputBlocked: false,
          skills: _skills,
          onSend: onSend ?? (_) {},
          onStop: () {},
        ),
      ),
    );

Future<void> _type(WidgetTester tester, String text) async {
  await tester.tap(find.byType(TextField));
  await tester.pump();
  await tester.enterText(find.byType(TextField), text);
  await tester.pump();
}

Future<void> _press(WidgetTester tester, LogicalKeyboardKey key) async {
  await tester.sendKeyEvent(key);
  await tester.pump();
}

String _draft(WidgetTester tester) =>
    tester.widget<TextField>(find.byType(TextField)).controller!.text;

/// Keyboard navigation of the slash picker.
///
/// The picker is only usable if it can be driven without the mouse, and the
/// widget tests elsewhere in this suite drive it by tapping — which exercises
/// none of the key handling. These send real key events.
void main() {
  group('slash picker keyboard', () {
    testWidgets('Enter accepts the highlighted skill', (tester) async {
      await tester.pumpWidget(_host());
      await _type(tester, '/com');
      await _press(tester, LogicalKeyboardKey.enter);
      expect(_draft(tester), '/commit ');
    });

    testWidgets('Tab accepts the highlighted skill', (tester) async {
      await tester.pumpWidget(_host());
      await _type(tester, '/rev');
      await _press(tester, LogicalKeyboardKey.tab);
      expect(_draft(tester), '/review ');
    });

    testWidgets('arrow down moves the selection before accepting',
        (tester) async {
      await tester.pumpWidget(_host());
      await _type(tester, '/rev'); // matches review, then code-review
      await _press(tester, LogicalKeyboardKey.arrowDown);
      await _press(tester, LogicalKeyboardKey.enter);
      expect(_draft(tester), '/code-review ');
    });

    testWidgets('arrow up wraps to the last match', (tester) async {
      await tester.pumpWidget(_host());
      await _type(tester, '/rev');
      await _press(tester, LogicalKeyboardKey.arrowUp);
      await _press(tester, LogicalKeyboardKey.enter);
      expect(_draft(tester), '/code-review ');
    });

    testWidgets('arrow down wraps back to the first match', (tester) async {
      await tester.pumpWidget(_host());
      await _type(tester, '/rev');
      await _press(tester, LogicalKeyboardKey.arrowDown);
      await _press(tester, LogicalKeyboardKey.arrowDown);
      await _press(tester, LogicalKeyboardKey.enter);
      expect(_draft(tester), '/review ');
    });

    testWidgets('Escape dismisses the picker without altering the draft',
        (tester) async {
      await tester.pumpWidget(_host());
      await _type(tester, '/com');
      expect(find.text('Write a commit'), findsOneWidget);

      await _press(tester, LogicalKeyboardKey.escape);

      expect(find.text('Write a commit'), findsNothing);
      expect(_draft(tester), '/com');
    });

    testWidgets('Enter after dismissal does not accept a skill',
        (tester) async {
      await tester.pumpWidget(_host());
      await _type(tester, '/com');
      await _press(tester, LogicalKeyboardKey.escape);
      await _press(tester, LogicalKeyboardKey.enter);
      expect(_draft(tester), '/com', reason: 'the picker is shut, so Enter is not its Enter');
    });

    testWidgets('a fresh slash token re-opens after a dismissal',
        (tester) async {
      await tester.pumpWidget(_host());
      await _type(tester, '/com');
      await _press(tester, LogicalKeyboardKey.escape);
      expect(find.text('Write a commit'), findsNothing);

      await _type(tester, 'now a new thought');
      await _type(tester, 'now a new thought /rev');

      expect(find.text('Review the diff'), findsOneWidget);
    });
  });

  group('send shortcut', () {
    testWidgets('Ctrl+Enter sends when the picker is closed', (tester) async {
      final sent = <String>[];
      await tester.pumpWidget(_host(onSend: sent.add));
      await _type(tester, 'ship it');

      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await _press(tester, LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);

      expect(sent, ['ship it']);
    });

    testWidgets('Ctrl+Enter steers a running turn', (tester) async {
      final sent = <String>[];
      await tester.pumpWidget(_host(streaming: true, onSend: sent.add));
      await _type(tester, 'try the other branch');

      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await _press(tester, LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);

      expect(sent, ['try the other branch']);
    });

    testWidgets('a bare Enter does not send', (tester) async {
      final sent = <String>[];
      await tester.pumpWidget(_host(onSend: sent.add));
      await _type(tester, 'still writing');
      await _press(tester, LogicalKeyboardKey.enter);
      expect(sent, isEmpty, reason: 'Enter inserts a newline; only the chord sends');
    });
  });
}
