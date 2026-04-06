import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter/material.dart';

class ToolCallBlock extends StatefulWidget {
  final ToolCall toolCall;

  const ToolCallBlock({super.key, required this.toolCall});

  @override
  State<ToolCallBlock> createState() => _ToolCallBlockState();
}

class _ToolCallBlockState extends State<ToolCallBlock> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tool = widget.toolCall;

    final statusColor = switch (tool.status) {
      ToolCallStatus.running => theme.colorScheme.primary,
      ToolCallStatus.done => Colors.green,
      ToolCallStatus.error => theme.colorScheme.error,
    };

    final statusLabel = switch (tool.status) {
      ToolCallStatus.running => 'Running...',
      ToolCallStatus.done => 'Done',
      ToolCallStatus.error => 'Error',
    };

    return Container(
      margin: const EdgeInsets.symmetric(vertical: 4),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerLow,
        border: Border.all(color: theme.dividerColor),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          InkWell(
            onTap: () => setState(() => _expanded = !_expanded),
            borderRadius: BorderRadius.circular(8),
            child: Padding(
              padding: const EdgeInsets.all(8),
              child: Row(
                children: [
                  Icon(
                    _expanded ? Icons.expand_more : Icons.chevron_right,
                    size: 16,
                  ),
                  const SizedBox(width: 4),
                  Text(
                    tool.name,
                    style: theme.textTheme.bodySmall?.copyWith(
                      fontFamily: 'SF Mono',
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    statusLabel,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: statusColor,
                    ),
                  ),
                  if (tool.status == ToolCallStatus.running) ...[
                    const SizedBox(width: 4),
                    SizedBox(
                      width: 10,
                      height: 10,
                      child: CircularProgressIndicator(strokeWidth: 1.5),
                    ),
                  ],
                ],
              ),
            ),
          ),
          if (_expanded && tool.result != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
              child: SelectableText(
                tool.result!,
                style: theme.textTheme.bodySmall?.copyWith(
                  fontFamily: 'SF Mono',
                  fontSize: 11,
                ),
              ),
            ),
        ],
      ),
    );
  }
}
