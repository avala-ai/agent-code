import 'package:agent_code_client/models/chat_message.dart';
import 'package:test/test.dart';

void main() {
  group('ChatMessage', () {
    test('user message has correct role', () {
      final msg = ChatMessage.user('hello');
      expect(msg.role, 'user');
      expect(msg.content, 'hello');
      expect(msg.id, isNotEmpty);
    });

    test('assistant message starts empty', () {
      final msg = ChatMessage.assistant();
      expect(msg.role, 'assistant');
      expect(msg.content, '');
      expect(msg.toolCalls, isEmpty);
      expect(msg.thinking, isNull);
    });

    test('generates unique IDs', () {
      final a = ChatMessage.user('a');
      final b = ChatMessage.user('b');
      expect(a.id, isNot(equals(b.id)));
    });

    test('appendContent returns a new instance, leaving the original intact', () {
      final msg = ChatMessage.assistant();
      final grown = msg.appendContent('hello').appendContent(' world');
      expect(grown.content, 'hello world');
      expect(msg.content, isEmpty);
      expect(grown.id, msg.id);
    });

    test('appendContent with empty text returns the same instance', () {
      final msg = ChatMessage.assistant();
      expect(identical(msg.appendContent(''), msg), isTrue);
    });

    test('appendThinking accumulates onto a null start', () {
      final msg = ChatMessage.assistant();
      final thought = msg.appendThinking('rea').appendThinking('soning');
      expect(thought.thinking, 'reasoning');
      expect(msg.thinking, isNull);
    });

    test('toolCalls is unmodifiable', () {
      final msg = ChatMessage.assistant();
      expect(() => msg.toolCalls.add(ToolCall(name: 'bash')),
          throwsUnsupportedError);
    });

    test('addToolCall returns a new instance with the call appended', () {
      final msg = ChatMessage.assistant();
      final withCall = msg.addToolCall(ToolCall(name: 'bash'));
      expect(withCall.toolCalls, hasLength(1));
      expect(msg.toolCalls, isEmpty);
    });

    test('replaceToolCall swaps by index', () {
      final msg = ChatMessage.assistant().addToolCall(ToolCall(name: 'bash'));
      final done = msg.replaceToolCall(
        0,
        msg.toolCalls.first.copyWith(status: ToolCallStatus.done),
      );
      expect(done.toolCalls.first.status, ToolCallStatus.done);
      expect(msg.toolCalls.first.status, ToolCallStatus.running);
    });

    test('replaceToolCall ignores an out-of-range index', () {
      final msg = ChatMessage.assistant();
      expect(identical(msg.replaceToolCall(0, ToolCall(name: 'x')), msg), isTrue);
    });

    test('copyWith preserves id and timestamp', () {
      final msg = ChatMessage.user('hi');
      final copy = msg.copyWith(content: 'bye');
      expect(copy.id, msg.id);
      expect(copy.timestamp, msg.timestamp);
      expect(copy.role, 'user');
      expect(copy.content, 'bye');
    });
  });

  group('ToolCall', () {
    test('defaults to running status', () {
      final tool = ToolCall(name: 'bash');
      expect(tool.status, ToolCallStatus.running);
      expect(tool.id, isNotEmpty);
    });

    test('copyWith returns a new instance with the new status', () {
      final tool = ToolCall(name: 'bash');
      final done = tool.copyWith(status: ToolCallStatus.done);
      expect(done.status, ToolCallStatus.done);
      expect(tool.status, ToolCallStatus.running);
      expect(done.id, tool.id);
      expect(done.name, 'bash');
    });

    test('carries input args and result content', () {
      final tool = ToolCall(name: 'Bash', input: const {'command': 'ls'});
      expect(tool.input!['command'], 'ls');
      expect(tool.isRunning, isTrue);
      final done = tool.copyWith(
        status: ToolCallStatus.done,
        result: 'a.txt\nb.txt',
      );
      expect(done.result, contains('a.txt'));
      expect(done.input!['command'], 'ls');
      expect(done.isRunning, isFalse);
    });

    test('generates unique IDs', () {
      final a = ToolCall(name: 'bash');
      final b = ToolCall(name: 'bash');
      expect(a.id, isNot(equals(b.id)));
    });
  });
}
