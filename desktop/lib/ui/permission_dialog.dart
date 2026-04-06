import 'package:flutter/material.dart';

import '../bloc/chat_state.dart';

class PermissionDialogWidget extends StatelessWidget {
  final PermissionRequest permission;
  final void Function(dynamic requestId, String decision) onRespond;

  const PermissionDialogWidget({
    super.key,
    required this.permission,
    required this.onRespond,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(20),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainerHigh,
        border: Border(top: BorderSide(color: theme.dividerColor)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Permission Required',
            style: theme.textTheme.titleSmall,
          ),
          const SizedBox(height: 4),
          Text(
            permission.toolName,
            style: theme.textTheme.bodyMedium?.copyWith(
              fontFamily: 'SF Mono',
              color: theme.colorScheme.primary,
            ),
          ),
          if (permission.inputPreview.isNotEmpty) ...[
            const SizedBox(height: 8),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(10),
              decoration: BoxDecoration(
                color: theme.colorScheme.surfaceContainerLow,
                borderRadius: BorderRadius.circular(8),
              ),
              constraints: const BoxConstraints(maxHeight: 150),
              child: SingleChildScrollView(
                child: SelectableText(
                  permission.inputPreview,
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontFamily: 'SF Mono',
                    fontSize: 11,
                  ),
                ),
              ),
            ),
          ],
          const SizedBox(height: 12),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              OutlinedButton(
                onPressed: () =>
                    onRespond(permission.requestId, 'deny'),
                style: OutlinedButton.styleFrom(
                  foregroundColor: theme.colorScheme.error,
                ),
                child: const Text('Deny'),
              ),
              const SizedBox(width: 8),
              OutlinedButton(
                onPressed: () =>
                    onRespond(permission.requestId, 'allow_once'),
                child: const Text('Allow Once'),
              ),
              const SizedBox(width: 8),
              FilledButton(
                onPressed: () =>
                    onRespond(permission.requestId, 'allow_session'),
                child: const Text('Allow for Session'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
