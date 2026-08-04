import 'package:flutter/material.dart';

import '../bloc/timeline.dart';
import 'app_theme.dart';

/// A single tool invocation, rendered as one quiet line rather than a card.
///
/// A turn can run dozens of tools; a bordered box each turns the transcript
/// into a wall of chrome. The row states what ran and how it went, and expands
/// on tap for the output.
class ToolCallRow extends StatefulWidget {
  final ToolRow row;
  final bool turnComplete;

  const ToolCallRow({
    super.key,
    required this.row,
    required this.turnComplete,
  });

  @override
  State<ToolCallRow> createState() => _ToolCallRowState();
}

class _ToolCallRowState extends State<ToolCallRow> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tokens = AppTokens.of(context);
    final row = widget.row;
    final kind = row.kindFor(turnComplete: widget.turnComplete);
    final result = row.call.result;
    final canExpand = result != null && result.trim().isNotEmpty;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        InkWell(
          onTap: canExpand ? () => setState(() => _expanded = !_expanded) : null,
          borderRadius: BorderRadius.circular(tokens.radiusSm),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 4, horizontal: 4),
            child: Row(
              children: [
                _StatusGlyph(kind: kind, color: _colorFor(theme, kind)),
                const SizedBox(width: 8),
                Text(
                  row.name,
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontWeight: FontWeight.w600,
                    color: theme.colorScheme.onSurface,
                  ),
                ),
                if (row.detail != null) ...[
                  const SizedBox(width: 8),
                  Expanded(
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
                ] else
                  const Spacer(),
                if (row.attempts > 1) ...[
                  const SizedBox(width: 8),
                  _AttemptsBadge(count: row.attempts),
                ],
                if (canExpand)
                  Icon(
                    _expanded ? Icons.expand_more : Icons.chevron_right,
                    size: 16,
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
              ],
            ),
          ),
        ),
        if (_expanded && canExpand)
          Container(
            width: double.infinity,
            margin: const EdgeInsets.only(left: 20, bottom: 6),
            padding: const EdgeInsets.all(10),
            decoration: BoxDecoration(
              color: tokens.codeBackground,
              borderRadius: BorderRadius.circular(tokens.radiusSm),
            ),
            child: SelectableText(
              result,
              style: theme.textTheme.bodySmall?.copyWith(
                fontFamily: tokens.monoFamily,
                fontSize: 11,
              ),
            ),
          ),
      ],
    );
  }

  Color _colorFor(ThemeData theme, ToolRowKind kind) => switch (kind) {
        ToolRowKind.running => theme.colorScheme.primary,
        ToolRowKind.ok => theme.colorScheme.onSurfaceVariant,
        ToolRowKind.failed => theme.colorScheme.error,
        ToolRowKind.attempted => theme.colorScheme.onSurfaceVariant,
      };
}

class _StatusGlyph extends StatelessWidget {
  final ToolRowKind kind;
  final Color color;

  const _StatusGlyph({required this.kind, required this.color});

  @override
  Widget build(BuildContext context) {
    if (kind == ToolRowKind.running) {
      return SizedBox(
        width: 12,
        height: 12,
        child: CircularProgressIndicator(strokeWidth: 1.5, color: color),
      );
    }
    return Icon(
      switch (kind) {
        ToolRowKind.ok => Icons.check,
        ToolRowKind.failed => Icons.close,
        ToolRowKind.attempted => Icons.remove,
        ToolRowKind.running => Icons.circle,
      },
      size: 12,
      color: color,
    );
  }
}

class _AttemptsBadge extends StatelessWidget {
  final int count;

  const _AttemptsBadge({required this.count});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(999),
      ),
      child: Text(
        '$count×',
        style: theme.textTheme.labelSmall?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}

/// The collapsed stand-in for a finished turn's tool activity — qm's
/// "7 tool calls ›" line. Expands to the full list of rows.
class ToolCallSummary extends StatefulWidget {
  final List<ToolRow> rows;
  final int callCount;
  final bool turnComplete;

  const ToolCallSummary({
    super.key,
    required this.rows,
    required this.callCount,
    required this.turnComplete,
  });

  @override
  State<ToolCallSummary> createState() => _ToolCallSummaryState();
}

class _ToolCallSummaryState extends State<ToolCallSummary> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tokens = AppTokens.of(context);
    final failed = widget.rows
        .where((r) => r.kindFor(turnComplete: widget.turnComplete) == ToolRowKind.failed)
        .length;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        InkWell(
          onTap: () => setState(() => _expanded = !_expanded),
          borderRadius: BorderRadius.circular(tokens.radiusSm),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 4, horizontal: 4),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  widget.callCount == 1 ? '1 tool call' : '${widget.callCount} tool calls',
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
                if (failed > 0) ...[
                  const SizedBox(width: 8),
                  Text(
                    '$failed failed',
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.error,
                    ),
                  ),
                ],
                const SizedBox(width: 2),
                Icon(
                  _expanded ? Icons.expand_more : Icons.chevron_right,
                  size: 16,
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ],
            ),
          ),
        ),
        if (_expanded)
          Padding(
            padding: const EdgeInsets.only(left: 8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (final row in widget.rows)
                  ToolCallRow(
                    key: ValueKey(row.call.id),
                    row: row,
                    turnComplete: widget.turnComplete,
                  ),
              ],
            ),
          ),
      ],
    );
  }
}
