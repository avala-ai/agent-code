import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter/material.dart';

import 'tool_call_block.dart';
import 'thinking_block.dart';
import 'markdown_renderer.dart';

class MessageBubble extends StatelessWidget {
  final ChatMessage message;
  final bool streaming;

  const MessageBubble({
    super.key,
    required this.message,
    this.streaming = false,
  });

  @override
  Widget build(BuildContext context) {
    if (message.role == 'user') {
      return _UserBubble(content: message.content);
    }
    return _AssistantBubble(message: message, streaming: streaming);
  }
}

class _UserBubble extends StatelessWidget {
  final String content;

  const _UserBubble({required this.content});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Align(
      alignment: Alignment.centerRight,
      child: Container(
        constraints: BoxConstraints(
          maxWidth: MediaQuery.of(context).size.width * 0.7,
        ),
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          color: theme.colorScheme.primary,
          borderRadius: const BorderRadius.only(
            topLeft: Radius.circular(12),
            topRight: Radius.circular(12),
            bottomLeft: Radius.circular(12),
            bottomRight: Radius.circular(4),
          ),
        ),
        child: SelectableText(
          content,
          style: TextStyle(color: theme.colorScheme.onPrimary),
        ),
      ),
    );
  }
}

class _AssistantBubble extends StatelessWidget {
  final ChatMessage message;
  final bool streaming;

  const _AssistantBubble({required this.message, required this.streaming});

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (message.thinking != null && message.thinking!.isNotEmpty)
          ThinkingBlock(text: message.thinking!),
        for (final tool in message.toolCalls) ToolCallBlock(toolCall: tool),
        if (message.content.isNotEmpty)
          streaming
              ? SelectableText(message.content)
              : MarkdownRenderer(content: message.content),
      ],
    );
  }
}
