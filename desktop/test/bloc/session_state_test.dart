import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_desktop/bloc/session_state.dart';

void main() {
  group('SessionState', () {
    test('defaults to empty', () {
      const state = SessionState();
      expect(state.sessions, isEmpty);
      expect(state.activeSessionId, isNull);
      expect(state.error, isNull);
    });

    test('activeSession returns matching session', () {
      final ws = WsClient();
      const instance = AgentInstance(
        pid: 1,
        port: 4096,
        cwd: '/tmp',
        token: 't',
      );
      final session = SessionData(
        id: 'sess-1',
        instance: instance,
        wsClient: ws,
      );

      final state = SessionState(
        sessions: [session],
        activeSessionId: 'sess-1',
      );
      expect(state.activeSession, session);
      ws.dispose();
    });

    test('activeSession returns null when no match', () {
      const state = SessionState(
        sessions: [],
        activeSessionId: 'nonexistent',
      );
      expect(state.activeSession, isNull);
    });

    test('copyWith preserves values', () {
      const state = SessionState(
        activeSessionId: 'a',
        error: 'err',
      );
      final copy = state.copyWith();
      expect(copy.activeSessionId, 'a');
      expect(copy.error, 'err');
    });

    test('copyWith overrides activeSessionId', () {
      const state = SessionState(activeSessionId: 'a');
      final copy = state.copyWith(activeSessionId: 'b');
      expect(copy.activeSessionId, 'b');
    });

    test('copyWith clearError', () {
      const state = SessionState(error: 'fail');
      final copy = state.copyWith(clearError: true);
      expect(copy.error, isNull);
    });
  });
}
