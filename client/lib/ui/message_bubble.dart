import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter/material.dart';

import '../bloc/timeline.dart';
import 'app_theme.dart';
import 'markdown_renderer.dart';
import 'thinking_block.dart';
import 'tool_call_block.dart';

/// Above this many tool rows, a finished turn collapses them behind a summary
/// line. Below it, the rows are short enough to just show.
const int kToolCollapseThreshold = 3;

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
    if (message.isUser) return _UserBubble(content: message.content);
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
    final tokens = AppTokens.of(context);
    final timeline = buildTimeline(message, turnComplete: !streaming);
    final toolRows = timeline.toolRows.toList();

    // A finished turn with a lot of tool activity reads as one line; a live one
    // shows its rows so progress is visible as it happens.
    final collapseTools =
        !streaming && toolRows.length >= kToolCollapseThreshold;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final item in timeline.items)
          switch (item.kind) {
            TimelineItemKind.thinking => ThinkingBlock(text: item.text!),
            TimelineItemKind.tool => collapseTools
                ? const SizedBox.shrink()
                : Padding(
                    padding: const EdgeInsets.symmetric(vertical: 1),
                    child: ToolCallRow(
                      key: ValueKey(item.row!.call.id),
                      row: item.row!,
                      turnComplete: !streaming,
                    ),
                  ),
            TimelineItemKind.text => Padding(
                padding: EdgeInsets.only(top: toolRows.isEmpty ? 0 : 6),
                child: MarkdownRenderer(
                  content: item.text!,
                  streaming: streaming,
                ),
              ),
          },
        if (collapseTools)
          Padding(
            padding: const EdgeInsets.only(bottom: 4),
            child: ToolCallSummary(
              rows: toolRows,
              callCount: timeline.toolCallCount,
              turnComplete: true,
            ),
          ),
        if (streaming && timeline.lastRunningTool != null)
          Padding(
            padding: EdgeInsets.only(top: 4, left: tokens.radiusSm),
            child: _RunningToolLine(row: timeline.lastRunningTool!),
          ),
      ],
    );
  }
}

/// The live "what is it doing right now" line, shown while a tool is in flight.
class _RunningToolLine extends StatelessWidget {
  final ToolRow row;

  const _RunningToolLine({required this.row});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tokens = AppTokens.of(context);
    return Row(
      children: [
        SizedBox(
          width: 11,
          height: 11,
          child: CircularProgressIndicator(
            strokeWidth: 1.5,
            color: theme.colorScheme.primary,
          ),
        ),
        const SizedBox(width: 8),
        Text(
          row.name,
          style: theme.textTheme.bodySmall?.copyWith(
            color: theme.colorScheme.onSurfaceVariant,
          ),
        ),
        if (row.detail != null) ...[
          const SizedBox(width: 6),
          Flexible(
            child: Text(
              row.detail!,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodySmall?.copyWith(
                fontFamily: tokens.monoFamily,
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
        ],
      ],
    );
  }
}
