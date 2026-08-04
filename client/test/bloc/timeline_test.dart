import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_client_app/bloc/timeline.dart';

ToolCall _call(
  String name, {
  Map<String, dynamic>? input,
  ToolCallStatus status = ToolCallStatus.running,
  String? result,
}) =>
    ToolCall(name: name, input: input, status: status, result: result);

ChatMessage _message({
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

void main() {
  group('structure', () {
    test('an empty message produces no items', () {
      final t = buildTimeline(_message(), turnComplete: true);
      expect(t.items, isEmpty);
    });

    test('whitespace-only content is not a text item', () {
      final t = buildTimeline(_message(content: '   \n  '), turnComplete: true);
      expect(t.items, isEmpty);
    });

    test('thinking precedes tools, which precede text', () {
      final t = buildTimeline(
        _message(
          thinking: 'considering',
          tools: [_call('Bash', status: ToolCallStatus.done, result: 'ok')],
          content: 'Done.',
        ),
        turnComplete: true,
      );
      expect(t.items.map((i) => i.kind), [
        TimelineItemKind.thinking,
        TimelineItemKind.tool,
        TimelineItemKind.text,
      ]);
    });
  });

  group('row detail', () {
    test('prefers a shell command', () {
      final row = ToolRow(call: _call('Bash', input: {'command': 'ls -la'}));
      expect(row.detail, 'ls -la');
    });

    test('falls back to a file path', () {
      final row = ToolRow(call: _call('Read', input: {'file_path': '/tmp/n.txt'}));
      expect(row.detail, '/tmp/n.txt');
    });

    test('falls back to a search pattern', () {
      final row = ToolRow(call: _call('Grep', input: {'pattern': 'TODO'}));
      expect(row.detail, 'TODO');
    });

    test('is null with no recognized argument', () {
      expect(ToolRow(call: _call('Mystery', input: {'zzz': 1})).detail, isNull);
      expect(ToolRow(call: _call('Mystery')).detail, isNull);
      expect(ToolRow(call: _call('Mystery', input: const {})).detail, isNull);
    });

    test('ignores an empty string argument', () {
      final row = ToolRow(
        call: _call('Bash', input: {'command': '', 'file_path': '/a'}),
      );
      expect(row.detail, '/a');
    });
  });

  group('row kind', () {
    test('unresolved during a live turn is running', () {
      final row = ToolRow(call: _call('Bash'));
      expect(row.kindFor(turnComplete: false), ToolRowKind.running);
    });

    test('unresolved after the turn ends is attempted, not running', () {
      final row = ToolRow(call: _call('Bash'));
      expect(row.kindFor(turnComplete: true), ToolRowKind.attempted);
    });

    test('a clean result is ok', () {
      final row = ToolRow(
        call: _call('Bash', status: ToolCallStatus.done, result: 'a.txt'),
      );
      expect(row.kindFor(turnComplete: true), ToolRowKind.ok);
    });

    test('a protocol error is failed', () {
      final row = ToolRow(
        call: _call('Bash', status: ToolCallStatus.error, result: 'boom'),
      );
      expect(row.kindFor(turnComplete: true), ToolRowKind.failed);
    });

    test('a nonzero exit reads as failed even when the protocol says done', () {
      final row = ToolRow(
        call: _call('Bash', status: ToolCallStatus.done, result: 'exit code 1'),
      );
      expect(row.kindFor(turnComplete: true), ToolRowKind.failed);
    });

    test('a zero exit stays ok', () {
      final row = ToolRow(
        call: _call('Bash', status: ToolCallStatus.done, result: 'exit code 0'),
      );
      expect(row.kindFor(turnComplete: true), ToolRowKind.ok);
    });

    test('the word "exit" deep in long output does not flip the row', () {
      final row = ToolRow(
        call: _call('Bash',
            status: ToolCallStatus.done,
            result: '${'x' * 500}\nexited with 3'),
      );
      expect(row.kindFor(turnComplete: true), ToolRowKind.ok);
    });
  });

  group('retry collapsing', () {
    test('adjacent identical unresolved calls fold into one row', () {
      final t = buildTimeline(
        _message(tools: [
          _call('Bash', input: {'command': 'flaky'}),
          _call('Bash', input: {'command': 'flaky'}),
          _call('Bash', input: {'command': 'flaky'}),
        ]),
        turnComplete: true,
      );
      expect(t.items, hasLength(1));
      expect(t.items.single.row!.attempts, 3);
      expect(t.toolCallCount, 3, reason: 'folded rows still count individually');
    });

    test('resolved calls never fold, even with the same command', () {
      final t = buildTimeline(
        _message(tools: [
          _call('Bash',
              input: {'command': 'ls'},
              status: ToolCallStatus.done,
              result: 'a'),
          _call('Bash',
              input: {'command': 'ls'},
              status: ToolCallStatus.done,
              result: 'b'),
        ]),
        turnComplete: true,
      );
      expect(t.items, hasLength(2));
    });

    test('different commands do not fold', () {
      final t = buildTimeline(
        _message(tools: [
          _call('Bash', input: {'command': 'one'}),
          _call('Bash', input: {'command': 'two'}),
        ]),
        turnComplete: true,
      );
      expect(t.items, hasLength(2));
    });

    test('different tools with the same detail do not fold', () {
      final t = buildTimeline(
        _message(tools: [
          _call('Read', input: {'path': '/a'}),
          _call('Write', input: {'path': '/a'}),
        ]),
        turnComplete: true,
      );
      expect(t.items, hasLength(2));
    });

    test('non-adjacent repeats do not fold across a resolved call', () {
      final t = buildTimeline(
        _message(tools: [
          _call('Bash', input: {'command': 'flaky'}),
          _call('Bash',
              input: {'command': 'other'},
              status: ToolCallStatus.done,
              result: 'x'),
          _call('Bash', input: {'command': 'flaky'}),
        ]),
        turnComplete: true,
      );
      expect(t.items, hasLength(3));
    });

    test('nothing folds while the turn is still live', () {
      // Two identical unresolved calls may still resolve differently; folding
      // them early would erase a distinction that has not been made yet.
      final t = buildTimeline(
        _message(tools: [
          _call('Bash', input: {'command': 'flaky'}),
          _call('Bash', input: {'command': 'flaky'}),
        ]),
        turnComplete: false,
      );
      expect(t.items, hasLength(2));
    });
  });

  group('summary', () {
    test('counts every call across mixed rows', () {
      final t = buildTimeline(
        _message(tools: [
          _call('Read',
              input: {'path': '/a'},
              status: ToolCallStatus.done,
              result: 'x'),
          _call('Bash', input: {'command': 'flaky'}),
          _call('Bash', input: {'command': 'flaky'}),
          _call('Grep',
              input: {'pattern': 'z'},
              status: ToolCallStatus.done,
              result: 'y'),
        ]),
        turnComplete: true,
      );
      expect(t.items, hasLength(3), reason: 'the two flaky calls fold');
      expect(t.toolCallCount, 4, reason: 'but four calls really happened');
    });

    test('lastRunningTool finds the live row', () {
      final t = buildTimeline(
        _message(tools: [
          _call('Read',
              input: {'path': '/a'},
              status: ToolCallStatus.done,
              result: 'x'),
          _call('Bash', input: {'command': 'sleep 5'}),
        ]),
        turnComplete: false,
      );
      expect(t.lastRunningTool, isNotNull);
      expect(t.lastRunningTool!.detail, 'sleep 5');
    });

    test('lastRunningTool is null once everything resolved', () {
      final t = buildTimeline(
        _message(tools: [
          _call('Read',
              input: {'path': '/a'},
              status: ToolCallStatus.done,
              result: 'x'),
        ]),
        turnComplete: true,
      );
      expect(t.lastRunningTool, isNull);
    });
  });
}
