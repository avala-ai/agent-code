import 'package:agent_code_client/agent_code_client.dart';
import 'package:bloc_test/bloc_test.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mocktail/mocktail.dart';

import 'package:agent_code_desktop/bloc/chat_bloc.dart';
import 'package:agent_code_desktop/bloc/chat_event.dart';
import 'package:agent_code_desktop/bloc/chat_state.dart';

class MockWsClient extends Mock implements WsClient {}

void main() {
  late MockWsClient mockWs;

  setUp(() {
    mockWs = MockWsClient();
    // Stub the streams to prevent subscription errors.
    when(() => mockWs.notifications)
        .thenAnswer((_) => const Stream.empty());
    when(() => mockWs.incomingRequests)
        .thenAnswer((_) => const Stream.empty());
  });

  group('ChatBloc', () {
    blocTest<ChatBloc, ChatState>(
      'emits user + assistant messages on SendMessageRequested',
      build: () {
        when(() => mockWs.sendMessage(any()))
            .thenAnswer((_) async => JsonRpcResponse.success(1, {
                  'response': 'hello back',
                  'turn_count': 1,
                  'tools_used': <String>[],
                  'cost_usd': 0.001,
                }));
        when(() => mockWs.getStatus()).thenAnswer((_) async => StatusResponse(
              sessionId: 's1',
              model: 'test',
              cwd: '/tmp',
              turnCount: 1,
              messageCount: 2,
              costUsd: 0.001,
              planMode: false,
              version: '0.1.0',
            ));
        return ChatBloc(wsClient: mockWs);
      },
      act: (bloc) => bloc.add(const SendMessageRequested('hello')),
      wait: const Duration(milliseconds: 100),
      verify: (bloc) {
        expect(bloc.state.messages.length, 2); // user + assistant
        expect(bloc.state.messages[0].role, 'user');
        expect(bloc.state.messages[0].content, 'hello');
        expect(bloc.state.messages[1].role, 'assistant');
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles text_delta notification',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () => ChatState(
        messages: [ChatMessage.user('hi'), ChatMessage.assistant()],
        streaming: true,
      ),
      act: (bloc) => bloc.add(NotificationReceived(
        const JsonRpcNotification(
          method: 'events/text_delta',
          params: {'text': 'hello'},
        ),
      )),
      verify: (bloc) {
        expect(bloc.state.messages.last.content, contains('hello'));
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles tool_start notification',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () => ChatState(
        messages: [ChatMessage.user('hi'), ChatMessage.assistant()],
        streaming: true,
      ),
      act: (bloc) => bloc.add(NotificationReceived(
        const JsonRpcNotification(
          method: 'events/tool_start',
          params: {'name': 'Bash'},
        ),
      )),
      verify: (bloc) {
        expect(bloc.state.messages.last.toolCalls, hasLength(1));
        expect(bloc.state.messages.last.toolCalls.first.name, 'Bash');
        expect(bloc.state.messages.last.toolCalls.first.status,
            ToolCallStatus.running);
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles tool_result notification',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () {
        final msg = ChatMessage.assistant();
        msg.toolCalls.add(ToolCall(name: 'Bash'));
        return ChatState(messages: [msg], streaming: true);
      },
      act: (bloc) => bloc.add(NotificationReceived(
        const JsonRpcNotification(
          method: 'events/tool_result',
          params: {'name': 'Bash', 'is_error': false},
        ),
      )),
      verify: (bloc) {
        expect(bloc.state.messages.last.toolCalls.first.status,
            ToolCallStatus.done);
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles tool_result with error',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () {
        final msg = ChatMessage.assistant();
        msg.toolCalls.add(ToolCall(name: 'Bash'));
        return ChatState(messages: [msg], streaming: true);
      },
      act: (bloc) => bloc.add(NotificationReceived(
        const JsonRpcNotification(
          method: 'events/tool_result',
          params: {'name': 'Bash', 'is_error': true},
        ),
      )),
      verify: (bloc) {
        expect(bloc.state.messages.last.toolCalls.first.status,
            ToolCallStatus.error);
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles thinking notification',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () => ChatState(
        messages: [ChatMessage.assistant()],
        streaming: true,
      ),
      act: (bloc) => bloc.add(NotificationReceived(
        const JsonRpcNotification(
          method: 'events/thinking',
          params: {'text': 'reasoning...'},
        ),
      )),
      verify: (bloc) {
        expect(bloc.state.messages.last.thinking, contains('reasoning'));
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles done notification — sets streaming false',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () => ChatState(
        messages: [ChatMessage.assistant()],
        streaming: true,
      ),
      act: (bloc) => bloc.add(NotificationReceived(
        const JsonRpcNotification(method: 'events/done', params: {}),
      )),
      verify: (bloc) {
        expect(bloc.state.streaming, isFalse);
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles error notification — appends error text',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () => ChatState(
        messages: [ChatMessage.assistant()],
        streaming: true,
      ),
      act: (bloc) => bloc.add(NotificationReceived(
        const JsonRpcNotification(
          method: 'events/error',
          params: {'message': 'something broke'},
        ),
      )),
      verify: (bloc) {
        expect(bloc.state.messages.last.content, contains('something broke'));
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles error notification — creates assistant message if none exists',
      build: () => ChatBloc(wsClient: mockWs),
      act: (bloc) => bloc.add(NotificationReceived(
        const JsonRpcNotification(
          method: 'events/error',
          params: {'message': 'fail'},
        ),
      )),
      verify: (bloc) {
        expect(bloc.state.messages, isNotEmpty);
        expect(bloc.state.messages.last.role, 'assistant');
        expect(bloc.state.messages.last.content, contains('fail'));
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles PermissionRequestReceived',
      build: () => ChatBloc(wsClient: mockWs),
      act: (bloc) => bloc.add(PermissionRequestReceived(
        const JsonRpcRequest(
          id: 42,
          method: 'ask_permission',
          params: {'tool': 'Bash', 'input': 'rm -rf /'},
        ),
      )),
      verify: (bloc) {
        expect(bloc.state.pendingPermission, isNotNull);
        expect(bloc.state.pendingPermission!.requestId, 42);
        expect(bloc.state.pendingPermission!.toolName, 'Bash');
        expect(bloc.state.pendingPermission!.inputPreview, 'rm -rf /');
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles PermissionResponded — clears pending and calls ws',
      build: () {
        when(() => mockWs.respondPermission(any(), any())).thenReturn(null);
        return ChatBloc(wsClient: mockWs);
      },
      seed: () => ChatState(
        pendingPermission: const PermissionRequest(
          requestId: 42,
          toolName: 'Bash',
          inputPreview: 'ls',
        ),
      ),
      act: (bloc) =>
          bloc.add(const PermissionResponded(42, 'allow_once')),
      verify: (bloc) {
        expect(bloc.state.pendingPermission, isNull);
        verify(() => mockWs.respondPermission(42, 'allow_once')).called(1);
      },
    );

    blocTest<ChatBloc, ChatState>(
      'handles ConnectionLost — resets streaming and sets error',
      build: () => ChatBloc(wsClient: mockWs),
      seed: () => const ChatState(streaming: true),
      act: (bloc) => bloc.add(const ConnectionLost()),
      verify: (bloc) {
        expect(bloc.state.streaming, isFalse);
        expect(bloc.state.error, isNotNull);
        expect(bloc.state.error, contains('lost'));
      },
    );

    blocTest<ChatBloc, ChatState>(
      'ignores informational notifications without crashing',
      build: () => ChatBloc(wsClient: mockWs),
      act: (bloc) {
        for (final method in [
          'events/usage',
          'events/turn_complete',
          'events/warning',
          'events/compact',
        ]) {
          bloc.add(NotificationReceived(
            JsonRpcNotification(method: method, params: const {}),
          ));
        }
      },
      verify: (bloc) {
        // No crash, no state change.
        expect(bloc.state.messages, isEmpty);
      },
    );
  });
}
