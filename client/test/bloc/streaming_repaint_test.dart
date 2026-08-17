import 'package:agent_code_client/agent_code_client.dart';
import 'package:bloc_test/bloc_test.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mocktail/mocktail.dart';

import 'package:agent_code_client_app/bloc/chat_bloc.dart';
import 'package:agent_code_client_app/bloc/chat_event.dart';
import 'package:agent_code_client_app/bloc/chat_state.dart';

class MockWsClient extends Mock implements WsClient {}

NotificationReceived _notify(String method, Map<String, dynamic> params) =>
    NotificationReceived(JsonRpcNotification(method: method, params: params));

/// Regression tests for dropped repaints during a streaming turn.
///
/// These assert on the *emitted* state sequence, not on the bloc's final state.
/// A handler that mutates a message in place and re-emits produces the right
/// final state while emitting nothing at all — the transcript then sits frozen
/// until some unrelated field happens to change. Only `expect:` catches that.
void main() {
  late MockWsClient mockWs;

  setUp(() {
    mockWs = MockWsClient();
    when(() => mockWs.notifications).thenAnswer((_) => const Stream.empty());
    when(() => mockWs.incomingRequests).thenAnswer((_) => const Stream.empty());
  });

  ChatState streamingSeed() => ChatState(
        messages: [ChatMessage.user('hi'), ChatMessage.assistant()],
        streaming: true,
      );

  group('streaming emits a repaint per delta', () {
    blocTest<ChatBloc, ChatState>(
      'every text_delta emits a state',
      build: () => ChatBloc(wsClient: mockWs),
      seed: streamingSeed,
      act: (bloc) {
        bloc.add(_notify('events/text_delta', {'text': 'one '}));
        bloc.add(_notify('events/text_delta', {'text': 'two '}));
        bloc.add(_notify('events/text_delta', {'text': 'three'}));
      },
      expect: () => [
        isA<ChatState>().having((s) => s.messages.last.content, 'content', 'one '),
        isA<ChatState>()
            .having((s) => s.messages.last.content, 'content', 'one two '),
        isA<ChatState>()
            .having((s) => s.messages.last.content, 'content', 'one two three'),
      ],
    );

    blocTest<ChatBloc, ChatState>(
      'revision advances monotonically across deltas',
      build: () => ChatBloc(wsClient: mockWs),
      seed: streamingSeed,
      act: (bloc) {
        bloc.add(_notify('events/text_delta', {'text': 'a'}));
        bloc.add(_notify('events/text_delta', {'text': 'b'}));
      },
      expect: () => [
        isA<ChatState>().having((s) => s.revision, 'revision', 1),
        isA<ChatState>().having((s) => s.revision, 'revision', 2),
      ],
    );

    blocTest<ChatBloc, ChatState>(
      'every thinking delta emits a state',
      build: () => ChatBloc(wsClient: mockWs),
      seed: streamingSeed,
      act: (bloc) {
        bloc.add(_notify('events/thinking', {'text': 'rea'}));
        bloc.add(_notify('events/thinking', {'text': 'soning'}));
      },
      expect: () => [
        isA<ChatState>().having((s) => s.messages.last.thinking, 'thinking', 'rea'),
        isA<ChatState>()
            .having((s) => s.messages.last.thinking, 'thinking', 'reasoning'),
      ],
    );

    blocTest<ChatBloc, ChatState>(
      'tool_start emits a state and keeps the message length unchanged',
      build: () => ChatBloc(wsClient: mockWs),
      seed: streamingSeed,
      act: (bloc) => bloc.add(_notify('events/tool_start', {
        'name': 'Bash',
        'input': {'command': 'ls -la'},
      })),
      expect: () => [
        isA<ChatState>()
            .having((s) => s.messages.length, 'length', 2)
            .having((s) => s.messages.last.toolCalls.length, 'tools', 1)
            .having(
              (s) => s.messages.last.toolCalls.first.input!['command'],
              'input.command',
              'ls -la',
            ),
      ],
    );

    blocTest<ChatBloc, ChatState>(
      'tool_result emits a state carrying status and output',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () => ChatState(
        messages: [
          ChatMessage.assistant().addToolCall(ToolCall(name: 'Bash')),
        ],
        streaming: true,
      ),
      act: (bloc) => bloc.add(_notify('events/tool_result', {
        'name': 'Bash',
        'is_error': false,
        'content': 'a.txt\nb.txt',
      })),
      expect: () => [
        isA<ChatState>()
            .having((s) => s.messages.last.toolCalls.first.status, 'status',
                ToolCallStatus.done)
            .having((s) => s.messages.last.toolCalls.first.result, 'result',
                'a.txt\nb.txt'),
      ],
    );

    blocTest<ChatBloc, ChatState>(
      'a no-op delta does not emit',
      build: () => ChatBloc(wsClient: mockWs),
      seed: streamingSeed,
      act: (bloc) => bloc.add(_notify('events/text_delta', {'text': ''})),
      expect: () => <ChatState>[],
    );

    blocTest<ChatBloc, ChatState>(
      'a tool_result with no matching running call does not emit',
      build: () => ChatBloc(wsClient: mockWs),
      seed: streamingSeed,
      act: (bloc) => bloc.add(_notify('events/tool_result', {
        'name': 'Bash',
        'is_error': false,
      })),
      expect: () => <ChatState>[],
    );
  });

  group('transcript integrity', () {
    blocTest<ChatBloc, ChatState>(
      'a delta arriving with no assistant message creates one',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () => ChatState(messages: [ChatMessage.user('hi')]),
      act: (bloc) => bloc.add(_notify('events/text_delta', {'text': 'hello'})),
      expect: () => [
        isA<ChatState>()
            .having((s) => s.messages.length, 'length', 2)
            .having((s) => s.messages.last.content, 'content', 'hello')
            .having((s) => s.streaming, 'streaming', true),
      ],
    );

    blocTest<ChatBloc, ChatState>(
      'streaming updates never mutate earlier messages',
      build: () => ChatBloc(wsClient: mockWs),
      seed: streamingSeed,
      act: (bloc) => bloc.add(_notify('events/text_delta', {'text': 'reply'})),
      verify: (bloc) {
        expect(bloc.state.messages.first.content, 'hi');
        expect(bloc.state.messages.first.role, 'user');
      },
    );

    blocTest<ChatBloc, ChatState>(
      'done stops streaming without bumping revision',
      build: () => ChatBloc(wsClient: mockWs),
      seed: streamingSeed,
      act: (bloc) => bloc.add(_notify('events/done', {})),
      expect: () => [
        isA<ChatState>()
            .having((s) => s.streaming, 'streaming', false)
            .having((s) => s.revision, 'revision', 0),
      ],
    );
  });

  group('status refresh', () {
    blocTest<ChatBloc, ChatState>(
      'status lands in state after the turn ends',
      build: () {
        when(() => mockWs.getStatus()).thenAnswer((_) async => StatusResponse(
              sessionId: 's1',
              model: 'test',
              cwd: '/tmp',
              turnCount: 7,
              messageCount: 2,
              costUsd: 0.5,
              planMode: false,
              version: '0.1.0',
            ));
        return ChatBloc(wsClient: mockWs);
      },
      seed: streamingSeed,
      act: (bloc) => bloc.add(_notify('events/done', {})),
      wait: const Duration(milliseconds: 100),
      verify: (bloc) {
        expect(bloc.state.status, isNotNull);
        expect(bloc.state.status!.turnCount, 7);
      },
    );
  });

  group('inputBlocked', () {
    test('streaming alone does not block input', () {
      expect(const ChatState(streaming: true).inputBlocked, isFalse);
    });

    test('a pending approval blocks input', () {
      const state = ChatState(
        pendingPermission: PermissionRequest(
          requestId: 1,
          toolName: 'Bash',
          inputPreview: 'rm -rf /',
        ),
      );
      expect(state.inputBlocked, isTrue);
    });
  });
}
