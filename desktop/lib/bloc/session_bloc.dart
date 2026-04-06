import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter_bloc/flutter_bloc.dart';
import 'package:uuid/uuid.dart';

import 'session_event.dart';
import 'session_state.dart';

const _uuid = Uuid();

class SessionBloc extends Bloc<SessionEvent, SessionState> {
  final AgentManager agentManager;

  SessionBloc({required this.agentManager}) : super(const SessionState()) {
    on<CreateSessionRequested>(_onCreateSession);
    on<DestroySessionRequested>(_onDestroySession);
    on<SwitchSessionRequested>(_onSwitchSession);
    on<DiscoverSessionsRequested>(_onDiscoverSessions);
    on<ReconnectSessionRequested>(_onReconnectSession);
  }

  Future<void> _onCreateSession(
    CreateSessionRequested event,
    Emitter<SessionState> emit,
  ) async {
    try {
      final instance = await agentManager.spawn(event.cwd);
      final wsClient = WsClient();
      await wsClient.connect(instance.port, instance.token);

      final sessionId = _uuid.v4();
      final session = SessionData(
        id: sessionId,
        instance: instance,
        wsClient: wsClient,
      );

      emit(state.copyWith(
        sessions: [...state.sessions, session],
        activeSessionId: sessionId,
        clearError: true,
      ));
    } catch (e) {
      emit(state.copyWith(error: e.toString()));
    }
  }

  Future<void> _onDestroySession(
    DestroySessionRequested event,
    Emitter<SessionState> emit,
  ) async {
    final session =
        state.sessions.where((s) => s.id == event.sessionId).firstOrNull;
    if (session == null) return;

    await session.wsClient.dispose();
    await agentManager.kill(session.instance.pid);

    final remaining =
        state.sessions.where((s) => s.id != event.sessionId).toList();
    final newActive = state.activeSessionId == event.sessionId
        ? remaining.lastOrNull?.id
        : state.activeSessionId;

    emit(state.copyWith(sessions: remaining, activeSessionId: newActive));
  }

  void _onSwitchSession(
    SwitchSessionRequested event,
    Emitter<SessionState> emit,
  ) {
    emit(state.copyWith(activeSessionId: event.sessionId));
  }

  Future<void> _onDiscoverSessions(
    DiscoverSessionsRequested event,
    Emitter<SessionState> emit,
  ) async {
    // Discovery is informational, handled by the UI.
    // The UI calls reconnectSession for each found instance.
  }

  Future<void> _onReconnectSession(
    ReconnectSessionRequested event,
    Emitter<SessionState> emit,
  ) async {
    try {
      final wsClient = WsClient();
      await wsClient.connect(event.instance.port, event.instance.token);

      final sessionId = _uuid.v4();
      final session = SessionData(
        id: sessionId,
        instance: event.instance,
        wsClient: wsClient,
      );

      emit(state.copyWith(
        sessions: [...state.sessions, session],
        activeSessionId: sessionId,
        clearError: true,
      ));
    } catch (e) {
      emit(state.copyWith(error: 'Reconnect failed: $e'));
    }
  }

  @override
  Future<void> close() async {
    // Kill all agent processes on app shutdown.
    for (final session in state.sessions) {
      await session.wsClient.dispose();
    }
    await agentManager.killAll();
    return super.close();
  }
}
