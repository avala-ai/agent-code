import 'package:agent_code_client/agent_code_client.dart';
import 'package:equatable/equatable.dart';

abstract class ChatEvent extends Equatable {
  const ChatEvent();

  @override
  List<Object?> get props => [];
}

class SendMessageRequested extends ChatEvent {
  final String content;
  const SendMessageRequested(this.content);

  @override
  List<Object?> get props => [content];
}

class NotificationReceived extends ChatEvent {
  final JsonRpcNotification notification;
  const NotificationReceived(this.notification);

  // Params are part of identity: two text deltas carrying different text are
  // different events, and keying only on the method makes them compare equal.
  @override
  List<Object?> get props => [notification.method, notification.params];
}

class PermissionRequestReceived extends ChatEvent {
  final JsonRpcRequest request;
  const PermissionRequestReceived(this.request);

  @override
  List<Object?> get props => [request.id];
}

class PermissionResponded extends ChatEvent {
  final dynamic requestId;
  final String decision; // 'allow_once', 'allow_session', 'deny'
  const PermissionResponded(this.requestId, this.decision);

  @override
  List<Object?> get props => [requestId, decision];
}

class ConnectionLost extends ChatEvent {
  const ConnectionLost();
}

/// Carries a freshly fetched status into the state.
///
/// Status is refreshed asynchronously after a turn ends. Emitting it from
/// inside the notification handler is not possible — that handler has already
/// returned by the time the fetch resolves, and a bloc emitter is invalid once
/// its handler completes — so the result comes back as its own event.
class StatusRefreshed extends ChatEvent {
  final StatusResponse status;
  const StatusRefreshed(this.status);

  @override
  List<Object?> get props => [status.turnCount, status.costUsd];
}
