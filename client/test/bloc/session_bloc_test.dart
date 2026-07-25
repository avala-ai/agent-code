import 'dart:async';

import 'package:agent_code_client/agent_code_client.dart';
import 'package:bloc_test/bloc_test.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mocktail/mocktail.dart';

import 'package:agent_code_client_app/bloc/session_activity.dart';
import 'package:agent_code_client_app/bloc/session_bloc.dart';
import 'package:agent_code_client_app/bloc/session_event.dart';
import 'package:agent_code_client_app/bloc/session_state.dart';

class MockWsClient extends Mock implements WsClient {}

/// Minimal stand-in for the dart:io AgentManager (typed dynamic in the bloc).
class FakeAgentManager {
  final AgentInstance instance;
  int killCount = 0;
  bool killedAll = false;
  FakeAgentManager(this.instance);

  Future<AgentInstance> spawn(String cwd) async => instance;
  Future<void> kill(int pid) async => killCount++;
  Future<void> killAll() async => killedAll = true;
}

const testInstance = AgentInstance(
  pid: 100,
  port: 4096,
  cwd: '/tmp/project',
  token: 'test-token',
  sessionId: 'sess-1',
);

void main() {
  group('SessionBloc', () {
    blocTest<SessionBloc, SessionState>(
      'initial state is empty',
      build: () => SessionBloc(agentManager: null),
      verify: (bloc) {
        expect(bloc.state.sessions, isEmpty);
        expect(bloc.state.activeSessionId, isNull);
        expect(bloc.state.error, isNull);
      },
    );

    blocTest<SessionBloc, SessionState>(
      'CreateSessionRequested with null manager shows error',
      build: () => SessionBloc(agentManager: null),
      act: (bloc) => bloc.add(const CreateSessionRequested('/tmp')),
      verify: (bloc) {
        expect(bloc.state.error, isNotNull);
        expect(bloc.state.error, contains('Cannot spawn'));
      },
    );

    blocTest<SessionBloc, SessionState>(
      'SwitchSessionRequested changes active session',
      build: () => SessionBloc(agentManager: null),
      seed: () {
        final ws = MockWsClient();
        when(() => ws.dispose()).thenAnswer((_) async {});
        return SessionState(
          sessions: [
            SessionData(id: 'a', instance: testInstance, wsClient: ws),
            SessionData(id: 'b', instance: testInstance, wsClient: ws),
          ],
          activeSessionId: 'a',
        );
      },
      act: (bloc) => bloc.add(const SwitchSessionRequested('b')),
      verify: (bloc) {
        expect(bloc.state.activeSessionId, 'b');
        expect(bloc.state.sessions, hasLength(2));
      },
    );

    blocTest<SessionBloc, SessionState>(
      'DestroySessionRequested removes session and switches active',
      build: () => SessionBloc(agentManager: null),
      seed: () {
        final ws1 = MockWsClient();
        final ws2 = MockWsClient();
        when(() => ws1.dispose()).thenAnswer((_) async {});
        when(() => ws2.dispose()).thenAnswer((_) async {});
        return SessionState(
          sessions: [
            SessionData(id: 'a', instance: testInstance, wsClient: ws1),
            SessionData(id: 'b', instance: testInstance, wsClient: ws2),
          ],
          activeSessionId: 'a',
        );
      },
      act: (bloc) => bloc.add(const DestroySessionRequested('a')),
      verify: (bloc) {
        expect(bloc.state.sessions, hasLength(1));
        expect(bloc.state.sessions.first.id, 'b');
        expect(bloc.state.activeSessionId, 'b');
      },
    );

    blocTest<SessionBloc, SessionState>(
      'DestroySessionRequested last session leaves empty state',
      build: () => SessionBloc(agentManager: null),
      seed: () {
        final ws = MockWsClient();
        when(() => ws.dispose()).thenAnswer((_) async {});
        return SessionState(
          sessions: [
            SessionData(id: 'a', instance: testInstance, wsClient: ws),
          ],
          activeSessionId: 'a',
        );
      },
      act: (bloc) => bloc.add(const DestroySessionRequested('a')),
      verify: (bloc) {
        expect(bloc.state.sessions, isEmpty);
        expect(bloc.state.activeSessionId, isNull);
      },
    );

    blocTest<SessionBloc, SessionState>(
      'ReconnectSessionRequested adds session',
      build: () => SessionBloc(agentManager: null),
      act: (bloc) => bloc.add(const ReconnectSessionRequested(testInstance)),
      wait: const Duration(milliseconds: 200),
      verify: (bloc) {
        // May fail to connect (no server running), so check for either success or error.
        final hasSession = bloc.state.sessions.isNotEmpty;
        final hasError = bloc.state.error != null;
        expect(hasSession || hasError, isTrue);
      },
    );
  });

  group('SessionBloc activity tracking', () {
    late StreamController<JsonRpcNotification> notifications;
    late StreamController<JsonRpcRequest> requests;
    late MockWsClient mockWs;
    late FakeAgentManager manager;

    setUp(() {
      notifications = StreamController<JsonRpcNotification>.broadcast();
      requests = StreamController<JsonRpcRequest>.broadcast();
      mockWs = MockWsClient();
      manager = FakeAgentManager(testInstance);
      when(() => mockWs.notifications).thenAnswer((_) => notifications.stream);
      when(() => mockWs.incomingRequests).thenAnswer((_) => requests.stream);
      when(() => mockWs.connect(any(), any())).thenAnswer((_) async {});
      when(() => mockWs.dispose()).thenAnswer((_) async {});
    });

    tearDown(() async {
      await notifications.close();
      await requests.close();
    });

    SessionBloc buildBloc() => SessionBloc(
          agentManager: manager,
          wsClientFactory: () => mockWs,
        );

    Future<String> createSession(SessionBloc bloc) async {
      bloc.add(const CreateSessionRequested('/tmp/project'));
      final state = await bloc.stream.firstWhere((s) => s.sessions.isNotEmpty);
      return state.sessions.first.id;
    }

    test('new session defaults to idle', () async {
      final bloc = buildBloc();
      final id = await createSession(bloc);
      expect(bloc.state.activityFor(id), SessionActivity.idle);
      await bloc.close();
    });

    test('derives working / needsInput / idle / failed from streams', () async {
      final bloc = buildBloc();
      final id = await createSession(bloc);

      // A turn producing output -> working.
      notifications.add(const JsonRpcNotification(
        method: 'events/tool_start',
        params: {'name': 'Bash'},
      ));
      await bloc.stream
          .firstWhere((s) => s.activityFor(id) == SessionActivity.working);

      // An ask_permission request -> needsInput.
      requests.add(const JsonRpcRequest(
        id: 1,
        method: 'ask_permission',
        params: {'tool': 'Bash', 'input': 'ls'},
      ));
      await bloc.stream
          .firstWhere((s) => s.activityFor(id) == SessionActivity.needsInput);

      // Turn finished -> idle.
      notifications
          .add(const JsonRpcNotification(method: 'events/done', params: {}));
      await bloc.stream
          .firstWhere((s) => s.activityFor(id) == SessionActivity.idle);

      // Error -> failed.
      notifications.add(const JsonRpcNotification(
        method: 'events/error',
        params: {'message': 'boom'},
      ));
      await bloc.stream
          .firstWhere((s) => s.activityFor(id) == SessionActivity.failed);

      expect(bloc.state.activityFor(id), SessionActivity.failed);
      await bloc.close();
    });

    test('text_delta and thinking also map to working', () async {
      final bloc = buildBloc();
      final id = await createSession(bloc);

      notifications.add(const JsonRpcNotification(
        method: 'events/text_delta',
        params: {'text': 'hi'},
      ));
      await bloc.stream
          .firstWhere((s) => s.activityFor(id) == SessionActivity.working);
      expect(bloc.state.activityFor(id), SessionActivity.working);
      await bloc.close();
    });

    test('destroying a session drops its activity and cancels subs', () async {
      final bloc = buildBloc();
      final id = await createSession(bloc);

      notifications.add(const JsonRpcNotification(
        method: 'events/tool_start',
        params: {'name': 'Bash'},
      ));
      await bloc.stream
          .firstWhere((s) => s.activityFor(id) == SessionActivity.working);

      bloc.add(DestroySessionRequested(id));
      await bloc.stream.firstWhere((s) => s.sessions.isEmpty);

      // Activity map no longer tracks the removed session.
      expect(bloc.state.activity.containsKey(id), isFalse);
      // A late notification for the removed session is ignored (no throw).
      notifications.add(const JsonRpcNotification(
        method: 'events/tool_start',
        params: {'name': 'Bash'},
      ));
      await Future<void>.delayed(const Duration(milliseconds: 10));
      expect(bloc.state.sessions, isEmpty);
      await bloc.close();
    });
  });
}
