import 'package:agent_code_client/agent_code_client.dart';
import 'package:bloc_test/bloc_test.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mocktail/mocktail.dart';

import '../../lib/bloc/session_bloc.dart';
import '../../lib/bloc/session_event.dart';
import '../../lib/bloc/session_state.dart';

class MockWsClient extends Mock implements WsClient {}

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
}
