import 'package:uuid/uuid.dart';

const _uuid = Uuid();

/// A single message in a chat session.
class ChatMessage {
  final String id;
  final String role; // 'user' or 'assistant'
  String content;
  final List<ToolCall> toolCalls;
  String? thinking;
  final DateTime timestamp;

  ChatMessage({
    String? id,
    required this.role,
    this.content = '',
    List<ToolCall>? toolCalls,
    this.thinking,
    DateTime? timestamp,
  })  : id = id ?? _uuid.v4(),
        toolCalls = toolCalls ?? [],
        timestamp = timestamp ?? DateTime.now();

  ChatMessage.user(String content)
      : this(role: 'user', content: content);

  ChatMessage.assistant()
      : this(role: 'assistant');
}

/// A tool invocation tracked within an assistant message.
class ToolCall {
  final String id;
  final String name;
  ToolCallStatus status;
  String? result;

  ToolCall({
    String? id,
    required this.name,
    this.status = ToolCallStatus.running,
    this.result,
  }) : id = id ?? _uuid.v4();
}

enum ToolCallStatus { running, done, error }
