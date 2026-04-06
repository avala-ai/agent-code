import 'package:agent_code_client/agent_code_client.dart';
import 'package:equatable/equatable.dart';

class PermissionRequest {
  final dynamic requestId;
  final String toolName;
  final String inputPreview;

  const PermissionRequest({
    required this.requestId,
    required this.toolName,
    required this.inputPreview,
  });
}

class ChatState extends Equatable {
  final List<ChatMessage> messages;
  final bool streaming;
  final StatusResponse? status;
  final PermissionRequest? pendingPermission;
  final String? error;

  const ChatState({
    this.messages = const [],
    this.streaming = false,
    this.status,
    this.pendingPermission,
    this.error,
  });

  /// The current assistant message being streamed (last message if it's assistant).
  ChatMessage? get currentAssistantMessage {
    if (messages.isEmpty) return null;
    final last = messages.last;
    return last.role == 'assistant' && streaming ? last : null;
  }

  ChatState copyWith({
    List<ChatMessage>? messages,
    bool? streaming,
    StatusResponse? status,
    PermissionRequest? pendingPermission,
    bool clearPermission = false,
    String? error,
    bool clearError = false,
  }) =>
      ChatState(
        messages: messages ?? this.messages,
        streaming: streaming ?? this.streaming,
        status: status ?? this.status,
        pendingPermission:
            clearPermission ? null : (pendingPermission ?? this.pendingPermission),
        error: clearError ? null : (error ?? this.error),
      );

  @override
  List<Object?> get props =>
      [messages.length, streaming, status?.turnCount, pendingPermission?.requestId, error];
}
