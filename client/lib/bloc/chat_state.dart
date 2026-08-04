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

  /// Monotonic counter bumped whenever [messages] changes in a way the UI must
  /// repaint for — including growth of the streaming message's content, which
  /// changes neither the list length nor any other field here.
  ///
  /// Equality keys on this rather than on the message list itself: deep-comparing
  /// a growing transcript on every token is quadratic in the length of the turn.
  final int revision;

  const ChatState({
    this.messages = const [],
    this.streaming = false,
    this.status,
    this.pendingPermission,
    this.error,
    this.revision = 0,
  });

  /// The current assistant message being streamed (last message if it's assistant).
  ChatMessage? get currentAssistantMessage {
    if (messages.isEmpty) return null;
    final last = messages.last;
    return last.role == 'assistant' && streaming ? last : null;
  }

  /// True when the composer must refuse input: an approval is outstanding and
  /// the turn cannot advance until the user answers it. Streaming alone does
  /// *not* block input — a running turn can still be steered.
  bool get inputBlocked => pendingPermission != null;

  ChatState copyWith({
    List<ChatMessage>? messages,
    bool? streaming,
    StatusResponse? status,
    PermissionRequest? pendingPermission,
    bool clearPermission = false,
    String? error,
    bool clearError = false,
    bool bumpRevision = false,
  }) =>
      ChatState(
        messages: messages ?? this.messages,
        streaming: streaming ?? this.streaming,
        status: status ?? this.status,
        pendingPermission:
            clearPermission ? null : (pendingPermission ?? this.pendingPermission),
        error: clearError ? null : (error ?? this.error),
        revision: bumpRevision ? revision + 1 : revision,
      );

  @override
  List<Object?> get props => [
        messages.length,
        revision,
        streaming,
        status?.turnCount,
        pendingPermission?.requestId,
        error,
      ];
}
