import 'package:agent_code_client/agent_code_client.dart';
import 'package:equatable/equatable.dart';

import 'session_activity.dart';

class SessionData {
  final String id;
  final AgentInstance instance;
  final WsClient wsClient;

  const SessionData({
    required this.id,
    required this.instance,
    required this.wsClient,
  });
}

class SessionState extends Equatable {
  final List<SessionData> sessions;
  final String? activeSessionId;
  final String? error;

  /// Live activity per session id. Missing ids default to
  /// [SessionActivity.idle] via [activityFor].
  final Map<String, SessionActivity> activity;

  const SessionState({
    this.sessions = const [],
    this.activeSessionId,
    this.error,
    this.activity = const {},
  });

  SessionData? get activeSession =>
      sessions.where((s) => s.id == activeSessionId).firstOrNull;

  /// Activity for [id], defaulting to idle when unknown.
  SessionActivity activityFor(String id) =>
      activity[id] ?? SessionActivity.idle;

  SessionState copyWith({
    List<SessionData>? sessions,
    String? activeSessionId,
    String? error,
    Map<String, SessionActivity>? activity,
    bool clearError = false,
    bool clearActive = false,
  }) =>
      SessionState(
        sessions: sessions ?? this.sessions,
        activeSessionId:
            clearActive ? null : (activeSessionId ?? this.activeSessionId),
        error: clearError ? null : (error ?? this.error),
        activity: activity ?? this.activity,
      );

  @override
  List<Object?> get props =>
      [sessions.length, activeSessionId, error, activity];
}
