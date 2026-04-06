import 'package:agent_code_client/agent_code_client.dart';
import 'package:equatable/equatable.dart';

abstract class SessionEvent extends Equatable {
  const SessionEvent();

  @override
  List<Object?> get props => [];
}

class CreateSessionRequested extends SessionEvent {
  final String cwd;
  const CreateSessionRequested(this.cwd);

  @override
  List<Object?> get props => [cwd];
}

class DestroySessionRequested extends SessionEvent {
  final String sessionId;
  const DestroySessionRequested(this.sessionId);

  @override
  List<Object?> get props => [sessionId];
}

class SwitchSessionRequested extends SessionEvent {
  final String sessionId;
  const SwitchSessionRequested(this.sessionId);

  @override
  List<Object?> get props => [sessionId];
}

class DiscoverSessionsRequested extends SessionEvent {
  const DiscoverSessionsRequested();
}

class ReconnectSessionRequested extends SessionEvent {
  final AgentInstance instance;
  const ReconnectSessionRequested(this.instance);

  @override
  List<Object?> get props => [instance.pid];
}
