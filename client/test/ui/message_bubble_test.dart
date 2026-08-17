import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_client_app/ui/app_theme.dart';
import 'package:agent_code_client_app/ui/message_bubble.dart';

Widget _host(Widget child) => MaterialApp(
      theme: buildAppTheme(Brightness.light),
      home: Scaffold(body: SingleChildScrollView(child: child)),
    );

ChatMessage _assistant({
  String content = '',
  String? thinking,
  List<ToolCall> tools = const [],
}) {
  var msg = ChatMessage.assistant();
  if (thinking != null) msg = msg.appendThinking(thinking);
  for (final t in tools) {
    msg = msg.addToolCall(t);
  }
  return msg.appendContent(content);
}

ToolCall _done(String name, Map<String, dynamic> input, {String result = 'ok'}) =>
    ToolCall(
      name: name,
      input: input,
      status: ToolCallStatus.done,
      result: result,
    );

void main() {
  testWidgets('a user message renders its text', (tester) async {
    await tester.pumpWidget(_host(
      MessageBubble(message: ChatMessage.user('hello there')),
    ));
    expect(find.text('hello there'), findsOneWidget);
  });

  testWidgets('a few tool rows render inline with their detail', (tester) async {
    await tester.pumpWidget(_host(MessageBubble(
      message: _assistant(
        tools: [
          _done('Read', {'file_path': '/tmp/notes.txt'}),
          _done('Bash', {'command': 'ls -la'}),
        ],
        content: 'Done.',
      ),
    )));

    expect(find.text('Read'), findsOneWidget);
    expect(find.text('/tmp/notes.txt'), findsOneWidget);
    expect(find.text('Bash'), findsOneWidget);
    expect(find.text('ls -la'), findsOneWidget);
    expect(find.textContaining('tool calls'), findsNothing);
  });

  testWidgets('many tool rows on a finished turn collapse to a summary',
      (tester) async {
    await tester.pumpWidget(_host(MessageBubble(
      message: _assistant(
        tools: [
          _done('Read', {'file_path': '/a'}),
          _done('Read', {'file_path': '/b'}),
          _done('Bash', {'command': 'ls'}),
          _done('Grep', {'pattern': 'TODO'}),
        ],
        content: 'All set.',
      ),
    )));

    expect(find.text('4 tool calls'), findsOneWidget);
    expect(find.text('/a'), findsNothing);

    await tester.tap(find.text('4 tool calls'));
    await tester.pumpAndSettle();

    expect(find.text('/a'), findsOneWidget);
    expect(find.text('TODO'), findsOneWidget);
  });

  testWidgets('the summary reports failures', (tester) async {
    await tester.pumpWidget(_host(MessageBubble(
      message: _assistant(
        tools: [
          _done('Read', {'file_path': '/a'}),
          _done('Read', {'file_path': '/b'}),
          ToolCall(
            name: 'Bash',
            input: const {'command': 'false'},
            status: ToolCallStatus.error,
            result: 'boom',
          ),
        ],
        content: 'Partly done.',
      ),
    )));

    expect(find.text('3 tool calls'), findsOneWidget);
    expect(find.text('1 failed'), findsOneWidget);
  });

  testWidgets('a live turn shows rows rather than collapsing them',
      (tester) async {
    await tester.pumpWidget(_host(MessageBubble(
      streaming: true,
      message: _assistant(
        tools: [
          _done('Read', {'file_path': '/a'}),
          _done('Read', {'file_path': '/b'}),
          _done('Read', {'file_path': '/c'}),
        ],
      ),
    )));

    expect(find.textContaining('tool calls'), findsNothing);
    expect(find.text('/a'), findsOneWidget);
    expect(find.text('/c'), findsOneWidget);
  });

  testWidgets('a running tool shows a live status line', (tester) async {
    await tester.pumpWidget(_host(MessageBubble(
      streaming: true,
      message: _assistant(
        tools: [ToolCall(name: 'Bash', input: const {'command': 'sleep 5'})],
      ),
    )));

    // Once in the row itself and once in the live status line beneath it.
    expect(find.text('sleep 5'), findsNWidgets(2));
    expect(find.byType(CircularProgressIndicator), findsWidgets);
  });

  testWidgets('retried calls collapse into one row with an attempt count',
      (tester) async {
    var msg = ChatMessage.assistant();
    for (var i = 0; i < 3; i++) {
      msg = msg.addToolCall(
        ToolCall(name: 'Bash', input: const {'command': 'flaky'}),
      );
    }

    await tester.pumpWidget(_host(MessageBubble(message: msg)));

    expect(find.text('flaky'), findsOneWidget);
    expect(find.text('3×'), findsOneWidget);
  });

  testWidgets('tool output expands on tap', (tester) async {
    await tester.pumpWidget(_host(MessageBubble(
      message: _assistant(
        tools: [_done('Bash', {'command': 'ls'}, result: 'a.txt\nb.txt')],
      ),
    )));

    expect(find.textContaining('a.txt'), findsNothing);
    await tester.tap(find.text('ls'));
    await tester.pumpAndSettle();
    expect(find.textContaining('a.txt'), findsOneWidget);
  });

  testWidgets('thinking is collapsed by default and expands on tap',
      (tester) async {
    await tester.pumpWidget(_host(MessageBubble(
      message: _assistant(thinking: 'weighing options', content: 'Here goes.'),
    )));

    expect(find.text('Thinking'), findsOneWidget);
    expect(find.textContaining('weighing options'), findsNothing);

    await tester.tap(find.text('Thinking'));
    await tester.pumpAndSettle();
    expect(find.textContaining('weighing options'), findsOneWidget);
  });

  testWidgets('an empty assistant message renders nothing', (tester) async {
    await tester.pumpWidget(_host(MessageBubble(message: _assistant())));
    expect(find.byType(CircularProgressIndicator), findsNothing);
    expect(find.text('Thinking'), findsNothing);
  });
}
