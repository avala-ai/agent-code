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
  bool inputBlocked = false,
  List<SkillItem> skills = const [],
  void Function(String)? onSend,
  VoidCallback? onStop,
  String initialDraft = '',
  ValueChanged<String>? onDraftChanged,
}) =>
    MaterialApp(
      theme: buildAppTheme(Brightness.light),
      home: Scaffold(
        body: Composer(
          streaming: streaming,
          inputBlocked: inputBlocked,
          skills: skills,
          onSend: onSend ?? (_) {},
          onStop: onStop,
          initialDraft: initialDraft,
          onDraftChanged: onDraftChanged,
        ),
      ),
    );

void main() {
  group('steer while running', () {
    testWidgets('the input stays enabled during a turn', (tester) async {
      await tester.pumpWidget(_host(streaming: true));
      final field = tester.widget<TextField>(find.byType(TextField));
      expect(field.enabled, isTrue);
    });

    testWidgets('the placeholder invites a steer during a turn', (tester) async {
      await tester.pumpWidget(_host(streaming: true));
      expect(find.text('Steer the running task…'), findsOneWidget);
    });

    testWidgets('the placeholder is neutral when idle', (tester) async {
      await tester.pumpWidget(_host());
      expect(find.text('Ask anything'), findsOneWidget);
    });

    testWidgets('a pending approval disables the input', (tester) async {
      await tester.pumpWidget(_host(inputBlocked: true, streaming: true));
      final field = tester.widget<TextField>(find.byType(TextField));
      expect(field.enabled, isFalse);
      expect(find.text('Approve or deny to continue'), findsOneWidget);
    });

    testWidgets('typing during a turn sends a steer', (tester) async {
      final sent = <String>[];
      await tester.pumpWidget(_host(streaming: true, onSend: sent.add));

      await tester.enterText(find.byType(TextField), 'try the other branch');
      await tester.pump();
      await tester.tap(find.byType(IconButton));
      await tester.pump();

      expect(sent, ['try the other branch']);
    });

    testWidgets('an empty composer during a turn offers stop', (tester) async {
      var stopped = false;
      await tester.pumpWidget(
        _host(streaming: true, onStop: () => stopped = true),
      );

      expect(find.byIcon(Icons.stop), findsOneWidget);
      await tester.tap(find.byIcon(Icons.stop));
      expect(stopped, isTrue);
    });

    testWidgets('typing during a turn swaps stop for steer', (tester) async {
      await tester.pumpWidget(_host(streaming: true, onStop: () {}));
      expect(find.byIcon(Icons.stop), findsOneWidget);

      await tester.enterText(find.byType(TextField), 'wait');
      await tester.pump();

      expect(find.byIcon(Icons.stop), findsNothing);
      expect(find.byIcon(Icons.subdirectory_arrow_right), findsOneWidget);
    });
  });

  group('sending', () {
    testWidgets('send is disabled with an empty draft', (tester) async {
      await tester.pumpWidget(_host());
      final button = tester.widget<IconButton>(find.byType(IconButton));
      expect(button.onPressed, isNull);
    });

    testWidgets('whitespace alone does not enable send', (tester) async {
      await tester.pumpWidget(_host());
      await tester.enterText(find.byType(TextField), '   \n  ');
      await tester.pump();
      final button = tester.widget<IconButton>(find.byType(IconButton));
      expect(button.onPressed, isNull);
    });

    testWidgets('sending trims and clears the field', (tester) async {
      final sent = <String>[];
      await tester.pumpWidget(_host(onSend: sent.add));

      await tester.enterText(find.byType(TextField), '  hello  ');
      await tester.pump();
      await tester.tap(find.byType(IconButton));
      await tester.pump();

      expect(sent, ['hello']);
      expect(tester.widget<TextField>(find.byType(TextField)).controller!.text,
          isEmpty);
    });
  });

  group('slash picker', () {
    testWidgets('typing a slash opens the picker', (tester) async {
      await tester.pumpWidget(_host(skills: _skills));

      await tester.enterText(find.byType(TextField), '/');
      await tester.pump();

      expect(find.text('/commit'), findsOneWidget);
      expect(find.text('Write a commit'), findsOneWidget);
    });

    testWidgets('typing filters the list', (tester) async {
      await tester.pumpWidget(_host(skills: _skills));

      await tester.enterText(find.byType(TextField), '/rev');
      await tester.pump();

      expect(find.text('Review the diff'), findsOneWidget);
      expect(find.text('Deep review'), findsOneWidget);
      expect(find.text('Write a commit'), findsNothing);
    });

    testWidgets('ordinary prose does not open the picker', (tester) async {
      await tester.pumpWidget(_host(skills: _skills));

      await tester.enterText(find.byType(TextField), 'no slash here');
      await tester.pump();

      expect(find.text('Write a commit'), findsNothing);
    });

    testWidgets('tapping a skill inserts it with a trailing space',
        (tester) async {
      await tester.pumpWidget(_host(skills: _skills));

      await tester.enterText(find.byType(TextField), '/com');
      await tester.pump();
      await tester.tap(find.text('Write a commit'));
      await tester.pump();

      final controller =
          tester.widget<TextField>(find.byType(TextField)).controller!;
      expect(controller.text, '/commit ');
    });

    testWidgets('the picker closes once a skill is accepted', (tester) async {
      await tester.pumpWidget(_host(skills: _skills));

      await tester.enterText(find.byType(TextField), '/com');
      await tester.pump();
      await tester.tap(find.text('Write a commit'));
      await tester.pump();

      expect(find.text('Write a commit'), findsNothing);
    });

    testWidgets('no picker when the agent reports no skills', (tester) async {
      await tester.pumpWidget(_host());
      await tester.enterText(find.byType(TextField), '/');
      await tester.pump();
      expect(find.byType(ListView), findsNothing);
    });
  });

  group('drafts', () {
    testWidgets('an initial draft populates the field', (tester) async {
      await tester.pumpWidget(_host(initialDraft: 'half a thought'));
      expect(find.text('half a thought'), findsOneWidget);
    });

    testWidgets('changes are reported to the parent', (tester) async {
      final seen = <String>[];
      await tester.pumpWidget(_host(onDraftChanged: seen.add));

      await tester.enterText(find.byType(TextField), 'draft');
      await tester.pump();

      expect(seen.last, 'draft');
    });
  });

  group('paste', () {
    testWidgets('a large paste becomes a chip instead of filling the input',
        (tester) async {
      final long = 'x' * 3000;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async => call.method == 'Clipboard.getData'
            ? <String, dynamic>{'text': long}
            : null,
      );
      addTearDown(() => tester.binding.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null));

      await tester.pumpWidget(_host());
      await tester.tap(find.byType(TextField));
      await tester.pump();

      Actions.invoke(
        tester.element(find.byType(TextField)),
        const PasteTextIntent(SelectionChangedCause.keyboard),
      );
      await tester.pumpAndSettle();

      expect(find.text('Pasted text · 3.0k chars'), findsOneWidget);
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        isEmpty,
      );
    });

    testWidgets('a chipped paste is appended to the sent message',
        (tester) async {
      final long = 'y' * 3000;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async => call.method == 'Clipboard.getData'
            ? <String, dynamic>{'text': long}
            : null,
      );
      addTearDown(() => tester.binding.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null));

      final sent = <String>[];
      await tester.pumpWidget(_host(onSend: sent.add));
      await tester.tap(find.byType(TextField));
      await tester.pump();

      Actions.invoke(
        tester.element(find.byType(TextField)),
        const PasteTextIntent(SelectionChangedCause.keyboard),
      );
      await tester.pumpAndSettle();

      await tester.enterText(find.byType(TextField), 'summarize this');
      await tester.pump();
      await tester.tap(find.byIcon(Icons.arrow_upward));
      await tester.pump();

      expect(sent, hasLength(1));
      expect(sent.single, startsWith('summarize this'));
      expect(sent.single, contains(long));
    });

    testWidgets('a chip can be removed before sending', (tester) async {
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async => call.method == 'Clipboard.getData'
            ? <String, dynamic>{'text': 'z' * 3000}
            : null,
      );
      addTearDown(() => tester.binding.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, null));

      await tester.pumpWidget(_host());
      await tester.tap(find.byType(TextField));
      await tester.pump();

      Actions.invoke(
        tester.element(find.byType(TextField)),
        const PasteTextIntent(SelectionChangedCause.keyboard),
      );
      await tester.pumpAndSettle();
      expect(find.byType(InputChip), findsOneWidget);

      await tester.tap(find.byIcon(Icons.close));
      await tester.pumpAndSettle();
      expect(find.byType(InputChip), findsNothing);
    });
  });
}
