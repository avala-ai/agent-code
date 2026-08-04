import 'dart:convert';

import 'package:agent_code_client/agent_code_client.dart';
import 'package:bloc_test/bloc_test.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mocktail/mocktail.dart';

import 'package:agent_code_client_app/bloc/chat_bloc.dart';
import 'package:agent_code_client_app/bloc/chat_event.dart';
import 'package:agent_code_client_app/bloc/chat_state.dart';
import 'package:agent_code_client_app/bloc/timeline.dart';

class MockWsClient extends Mock implements WsClient {}

/// Verbatim JSON-RPC notification frames as `crates/cli/src/serve.rs` emits
/// them: the `SseEvent` enum serializes with `#[serde(tag = "type")]` and the
/// whole event object becomes the notification's `params`.
///
/// The rest of the suite constructs params by hand, which tests the bloc against
/// the shape this client *expects* rather than the shape the server *sends* —
/// a distinction that hides exactly the bug where a field is renamed or nested
/// differently on the wire. These frames are copied from the server's
/// serialization and its `tool_events_serialize_with_input_and_content` test.
const _textDelta = '''
{"jsonrpc":"2.0","method":"events/text_delta","params":{"type":"text_delta","text":"Looking at the file. "}}''';

const _thinking = '''
{"jsonrpc":"2.0","method":"events/thinking","params":{"type":"thinking","text":"The user wants the config."}}''';

const _toolStart = '''
{"jsonrpc":"2.0","method":"events/tool_start","params":{"type":"tool_start","name":"Bash","input":{"command":"ls -la"}}}''';

const _toolResult = '''
{"jsonrpc":"2.0","method":"events/tool_result","params":{"type":"tool_result","name":"Bash","is_error":false,"content":"a.txt\\nb.txt"}}''';

const _toolResultError = '''
{"jsonrpc":"2.0","method":"events/tool_result","params":{"type":"tool_result","name":"Bash","is_error":true,"content":"command not found"}}''';

const _usage = '''
{"jsonrpc":"2.0","method":"events/usage","params":{"type":"usage","input_tokens":1200,"output_tokens":340}}''';

const _turnComplete = '''
{"jsonrpc":"2.0","method":"events/turn_complete","params":{"type":"turn_complete","turn":3}}''';

const _compact = '''
{"jsonrpc":"2.0","method":"events/compact","params":{"type":"compact","freed_tokens":8000}}''';

const _warning = '''
{"jsonrpc":"2.0","method":"events/warning","params":{"type":"warning","message":"context is filling up"}}''';

const _error = '''
{"jsonrpc":"2.0","method":"events/error","params":{"type":"error","message":"provider returned 429"}}''';

const _done = '''
{"jsonrpc":"2.0","method":"events/done","params":{"type":"done","response":"Looking at the file. ","turn_count":1,"tools_used":["Bash"],"cost_usd":0.0021}}''';

/// Parses a server frame the way [WsClient] does before handing it to the bloc.
NotificationReceived _frame(String raw) {
  final json = jsonDecode(raw) as Map<String, dynamic>;
  return NotificationReceived(JsonRpcNotification(
    method: json['method'] as String,
    params: json['params'] as Map<String, dynamic>,
  ));
}

void main() {
  late MockWsClient mockWs;

  setUp(() {
    mockWs = MockWsClient();
    when(() => mockWs.notifications).thenAnswer((_) => const Stream.empty());
    when(() => mockWs.incomingRequests).thenAnswer((_) => const Stream.empty());
  });

  group('server wire format', () {
    blocTest<ChatBloc, ChatState>(
      'a full turn, replayed frame for frame, produces the expected transcript',
      build: () => ChatBloc(wsClient: mockWs),
      act: (bloc) {
        for (final raw in [
          _thinking,
          _toolStart,
          _toolResult,
          _textDelta,
          _usage,
          _turnComplete,
          _done,
        ]) {
          bloc.add(_frame(raw));
        }
      },
      verify: (bloc) {
        final msg = bloc.state.messages.single;
        expect(msg.isAssistant, isTrue);
        expect(msg.thinking, 'The user wants the config.');
        expect(msg.content, 'Looking at the file. ');
        expect(bloc.state.streaming, isFalse);

        final call = msg.toolCalls.single;
        expect(call.name, 'Bash');
        expect(call.input!['command'], 'ls -la',
            reason: 'tool_start input must survive the wire');
        expect(call.result, 'a.txt\nb.txt',
            reason: 'tool_result content must survive the wire');
        expect(call.status, ToolCallStatus.done);
      },
    );

    blocTest<ChatBloc, ChatState>(
      'an errored tool result lands as a failed row',
      build: () => ChatBloc(wsClient: mockWs),
      act: (bloc) {
        bloc.add(_frame(_toolStart));
        bloc.add(_frame(_toolResultError));
      },
      verify: (bloc) {
        final msg = bloc.state.messages.single;
        expect(msg.toolCalls.single.status, ToolCallStatus.error);

        final timeline = buildTimeline(msg, turnComplete: true);
        expect(
          timeline.toolRows.single.kindFor(turnComplete: true),
          ToolRowKind.failed,
        );
      },
    );

    blocTest<ChatBloc, ChatState>(
      'an error frame surfaces its message in the transcript',
      build: () => ChatBloc(wsClient: mockWs),
      act: (bloc) => bloc.add(_frame(_error)),
      verify: (bloc) {
        expect(bloc.state.messages.last.content, contains('provider returned 429'));
      },
    );

    blocTest<ChatBloc, ChatState>(
      'informational frames are absorbed without disturbing the transcript',
      build: () => ChatBloc(wsClient: mockWs),
      act: (bloc) {
        for (final raw in [_usage, _turnComplete, _compact, _warning]) {
          bloc.add(_frame(raw));
        }
      },
      expect: () => <ChatState>[],
      verify: (bloc) => expect(bloc.state.messages, isEmpty),
    );

    blocTest<ChatBloc, ChatState>(
      'the tool detail shown to the user comes from the real input payload',
      build: () => ChatBloc(wsClient: mockWs),
      act: (bloc) => bloc.add(_frame(_toolStart)),
      verify: (bloc) {
        final timeline = buildTimeline(
          bloc.state.messages.single,
          turnComplete: false,
        );
        expect(timeline.toolRows.single.detail, 'ls -la');
      },
    );

    test('every event the server can emit is a method the bloc handles', () {
      // Mirrors the match arms in serve.rs. A new SseEvent variant that the
      // client silently ignores is a rendering gap, so this list is the
      // reminder to update both sides together.
      const emitted = {
        'events/text_delta',
        'events/tool_start',
        'events/tool_result',
        'events/thinking',
        'events/turn_complete',
        'events/usage',
        'events/error',
        'events/compact',
        'events/warning',
        'events/done',
      };
      const handled = {
        'events/text_delta',
        'events/thinking',
        'events/tool_start',
        'events/tool_result',
        'events/done',
        'events/error',
        'events/usage',
        'events/turn_complete',
        'events/warning',
        'events/compact',
      };
      expect(emitted.difference(handled), isEmpty,
          reason: 'server emits an event the client does not handle');
    });
  });
}
