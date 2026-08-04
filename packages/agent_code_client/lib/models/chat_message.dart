import 'package:uuid/uuid.dart';

const _uuid = Uuid();

/// A single message in a chat session.
///
/// Immutable: streaming updates produce a new instance via [copyWith] rather
/// than mutating in place. In-place mutation defeats state-equality checks in
/// the UI layer, which silently drops repaints while a turn streams.
class ChatMessage {
  final String id;
  final String role; // 'user' or 'assistant'
  final String content;
  final List<ToolCall> toolCalls;
  final String? thinking;
  final DateTime timestamp;

  ChatMessage({
    String? id,
    required this.role,
    this.content = '',
    List<ToolCall>? toolCalls,
    this.thinking,
    DateTime? timestamp,
  })  : id = id ?? _uuid.v4(),
        toolCalls = List.unmodifiable(toolCalls ?? const []),
        timestamp = timestamp ?? DateTime.now();

  ChatMessage.user(String content) : this(role: 'user', content: content);

  ChatMessage.assistant() : this(role: 'assistant');

  bool get isUser => role == 'user';
  bool get isAssistant => role == 'assistant';

  ChatMessage copyWith({
    String? content,
    List<ToolCall>? toolCalls,
    String? thinking,
  }) =>
      ChatMessage(
        id: id,
        role: role,
        content: content ?? this.content,
        toolCalls: toolCalls ?? this.toolCalls,
        thinking: thinking ?? this.thinking,
        timestamp: timestamp,
      );

  /// Returns a copy with [text] appended to [content].
  ChatMessage appendContent(String text) =>
      text.isEmpty ? this : copyWith(content: content + text);

  /// Returns a copy with [text] appended to [thinking].
  ChatMessage appendThinking(String text) =>
      text.isEmpty ? this : copyWith(thinking: (thinking ?? '') + text);

  /// Returns a copy with [call] appended to [toolCalls].
  ChatMessage addToolCall(ToolCall call) =>
      copyWith(toolCalls: [...toolCalls, call]);

  /// Returns a copy with the tool call at [index] replaced by [call].
  ChatMessage replaceToolCall(int index, ToolCall call) {
    if (index < 0 || index >= toolCalls.length) return this;
    final next = List<ToolCall>.of(toolCalls);
    next[index] = call;
    return copyWith(toolCalls: next);
  }
}

/// A tool invocation tracked within an assistant message.
///
/// Immutable — see the note on [ChatMessage].
class ToolCall {
  final String id;
  final String name;
  final ToolCallStatus status;

  /// The tool's input arguments as sent by the agent, used to render a detail
  /// preview (the bash command, the edited file path, …).
  final Map<String, dynamic>? input;

  /// The tool's output, once it has produced one.
  final String? result;

  ToolCall({
    String? id,
    required this.name,
    this.status = ToolCallStatus.running,
    this.input,
    this.result,
  }) : id = id ?? _uuid.v4();

  bool get isRunning => status == ToolCallStatus.running;

  ToolCall copyWith({
    ToolCallStatus? status,
    Map<String, dynamic>? input,
    String? result,
  }) =>
      ToolCall(
        id: id,
        name: name,
        status: status ?? this.status,
        input: input ?? this.input,
        result: result ?? this.result,
      );
}

enum ToolCallStatus { running, done, error }
