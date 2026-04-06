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

  @override
  List<Object?> get props => [notification.method];
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
