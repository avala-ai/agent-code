import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter/material.dart';

class StatusBar extends StatelessWidget {
  final StatusResponse? status;
  final bool streaming;

  const StatusBar({super.key, required this.status, required this.streaming});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 6),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerLow,
        border: Border(top: BorderSide(color: theme.dividerColor)),
      ),
      child: Row(
        children: [
          // Connection indicator
          Container(
            width: 6,
            height: 6,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: streaming ? theme.colorScheme.primary : Colors.green,
            ),
          ),
          const SizedBox(width: 6),
          Text(
            streaming ? 'Streaming' : 'Ready',
            style: theme.textTheme.labelSmall,
          ),
          if (status != null) ...[
            const SizedBox(width: 16),
            Text(status!.model, style: theme.textTheme.labelSmall),
            const SizedBox(width: 16),
            Text(
              '\$${status!.costUsd.toStringAsFixed(4)}',
              style: theme.textTheme.labelSmall,
            ),
            const SizedBox(width: 16),
            Text(
              '${status!.turnCount} turns',
              style: theme.textTheme.labelSmall,
            ),
            const Spacer(),
            Text(
              'v${status!.version}',
              style: theme.textTheme.labelSmall,
            ),
          ],
        ],
      ),
    );
  }
}
