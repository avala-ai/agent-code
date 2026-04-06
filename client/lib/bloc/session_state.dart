import 'package:agent_code_client/agent_code_client.dart';
import 'package:equatable/equatable.dart';

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

  const SessionState({
    this.sessions = const [],
    this.activeSessionId,
    this.error,
  });

  SessionData? get activeSession =>
      sessions.where((s) => s.id == activeSessionId).firstOrNull;

  SessionState copyWith({
    List<SessionData>? sessions,
    String? activeSessionId,
    String? error,
    bool clearError = false,
  }) =>
      SessionState(
        sessions: sessions ?? this.sessions,
        activeSessionId: activeSessionId ?? this.activeSessionId,
        error: clearError ? null : (error ?? this.error),
      );

  @override
  List<Object?> get props => [sessions.length, activeSessionId, error];
}
