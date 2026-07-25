import 'dart:async';

import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter_bloc/flutter_bloc.dart';
import 'package:uuid/uuid.dart';

import 'session_activity.dart';
import 'session_event.dart';
import 'session_state.dart';

const _uuid = Uuid();

/// Builds a fresh [WsClient]. Injectable so tests can supply a controllable
/// double whose notification/request streams they drive directly.
typedef WsClientFactory = WsClient Function();

class SessionBloc extends Bloc<SessionEvent, SessionState> {
  /// The agent manager instance. Null on web (no dart:io process spawning).
  /// Typed as dynamic because AgentManager uses dart:io which is unavailable on web.
  final dynamic agentManager;

  final WsClientFactory _wsClientFactory;

  /// Per-session stream subscriptions (notifications + incoming requests),
  /// cancelled when the session is destroyed to avoid leaks.
  final Map<String, List<StreamSubscription<dynamic>>> _subscriptions = {};

  SessionBloc({
    required this.agentManager,
    WsClientFactory? wsClientFactory,
  })  : _wsClientFactory = wsClientFactory ?? WsClient.new,
        super(const SessionState()) {
    on<CreateSessionRequested>(_onCreateSession);
    on<DestroySessionRequested>(_onDestroySession);
    on<SwitchSessionRequested>(_onSwitchSession);
    on<DiscoverSessionsRequested>(_onDiscoverSessions);
    on<ReconnectSessionRequested>(_onReconnectSession);
    on<SessionActivityChanged>(_onActivityChanged);
  }

  Future<void> _onCreateSession(
    CreateSessionRequested event,
    Emitter<SessionState> emit,
  ) async {
    try {
      if (agentManager == null) {
        emit(state.copyWith(
            error: 'Cannot spawn agent processes on web. '
                'Connect to an existing agent instead.'));
        return;
      }
      final instance = await agentManager.spawn(event.cwd) as AgentInstance;
      final wsClient = _wsClientFactory();
      await wsClient.connect(instance.port, instance.token);

      final sessionId = _uuid.v4();
      final session = SessionData(
        id: sessionId,
        instance: instance,
        wsClient: wsClient,
      );

      _watchSession(session);

      emit(state.copyWith(
        sessions: [...state.sessions, session],
        activeSessionId: sessionId,
        activity: {...state.activity, sessionId: SessionActivity.idle},
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

    await _cancelSubscriptions(event.sessionId);
    await session.wsClient.dispose();
    if (agentManager != null) await agentManager.kill(session.instance.pid);

    final remaining =
        state.sessions.where((s) => s.id != event.sessionId).toList();
    final removingActive = state.activeSessionId == event.sessionId;
    final newActive =
        removingActive ? remaining.lastOrNull?.id : state.activeSessionId;
    final activity = {...state.activity}..remove(event.sessionId);

    emit(state.copyWith(
      sessions: remaining,
      activeSessionId: newActive,
      // copyWith can't otherwise set activeSessionId back to null.
      clearActive: removingActive && remaining.isEmpty,
      activity: activity,
    ));
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
      final wsClient = _wsClientFactory();
      await wsClient.connect(event.instance.port, event.instance.token);

      final sessionId = _uuid.v4();
      final session = SessionData(
        id: sessionId,
        instance: event.instance,
        wsClient: wsClient,
      );

      _watchSession(session);

      emit(state.copyWith(
        sessions: [...state.sessions, session],
        activeSessionId: sessionId,
        activity: {...state.activity, sessionId: SessionActivity.idle},
        clearError: true,
      ));
    } catch (e) {
      emit(state.copyWith(error: 'Reconnect failed: $e'));
    }
  }

  void _onActivityChanged(
    SessionActivityChanged event,
    Emitter<SessionState> emit,
  ) {
    // Ignore updates for sessions that were removed (or never existed).
    if (state.sessions.every((s) => s.id != event.sessionId)) return;
    if (state.activityFor(event.sessionId) == event.activity) return;

    emit(state.copyWith(
      activity: {...state.activity, event.sessionId: event.activity},
    ));
  }

  /// Subscribe to a session's notification and incoming-request streams,
  /// mapping WebSocket events to a [SessionActivity] via [SessionActivityChanged].
  void _watchSession(SessionData session) {
    final id = session.id;
    final subs = <StreamSubscription<dynamic>>[
      session.wsClient.notifications.listen((n) {
        final activity = _activityForNotification(n.method);
        if (activity != null) add(SessionActivityChanged(id, activity));
      }),
      session.wsClient.incomingRequests.listen((r) {
        if (r.method == 'ask_permission') {
          add(SessionActivityChanged(id, SessionActivity.needsInput));
        }
      }),
    ];
    _subscriptions[id] = subs;
  }

  /// Map a notification method to the activity it implies, or null to ignore.
  static SessionActivity? _activityForNotification(String method) {
    switch (method) {
      case 'events/tool_start':
      case 'events/text_delta':
      case 'events/thinking':
        return SessionActivity.working;
      case 'events/done':
      case 'events/turn_complete':
        return SessionActivity.idle;
      case 'events/error':
        return SessionActivity.failed;
      default:
        return null;
    }
  }

  Future<void> _cancelSubscriptions(String sessionId) async {
    final subs = _subscriptions.remove(sessionId);
    if (subs == null) return;
    for (final sub in subs) {
      await sub.cancel();
    }
  }

  @override
  Future<void> close() async {
    // Cancel all activity subscriptions and kill agent processes on shutdown.
    for (final subs in _subscriptions.values) {
      for (final sub in subs) {
        await sub.cancel();
      }
    }
    _subscriptions.clear();
    for (final session in state.sessions) {
      await session.wsClient.dispose();
    }
    if (agentManager != null) await agentManager.killAll();
    return super.close();
  }
}
