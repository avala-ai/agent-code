import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import '../bloc/session_bloc.dart';
import '../bloc/session_state.dart';
import 'chat_view.dart';
import 'sidebar.dart';

class AppShell extends StatelessWidget {
  const AppShell({super.key});

  @override
  Widget build(BuildContext context) {
    return BlocBuilder<SessionBloc, SessionState>(
      builder: (context, state) {
        return Row(
          children: [
            const SizedBox(width: 240, child: Sidebar()),
            const VerticalDivider(width: 1),
            Expanded(
              child: state.activeSession != null
                  ? ChatView(
                      key: ValueKey(state.activeSessionId),
                      session: state.activeSession!,
                    )
                  : _EmptyState(),
            ),
          ],
        );
      },
    );
  }
}

class _EmptyState extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              'Agent Code',
              style: theme.textTheme.headlineSmall,
            ),
            const SizedBox(height: 8),
            Text(
              'Create a new session to get started',
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
