import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import '../bloc/chat_bloc_registry.dart';
import '../bloc/session_bloc.dart';
import '../bloc/session_state.dart';
import 'app_theme.dart';
import 'chat_view.dart';
import 'sidebar.dart';

class AppShell extends StatefulWidget {
  const AppShell({super.key});

  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell> {
  /// Session state outlives the views that show it, so the registry is owned by
  /// the shell rather than by any one [ChatView].
  final ChatBlocRegistry _chatBlocs = ChatBlocRegistry();
  final Set<String> _knownSessionIds = {};

  bool _sidebarOpen = true;

  @override
  void dispose() {
    _chatBlocs.clear();
    super.dispose();
  }

  void _toggleSidebar() => setState(() => _sidebarOpen = !_sidebarOpen);

  /// Retires state for sessions that no longer exist. Their blocs hold live
  /// socket subscriptions, so leaving them behind leaks both memory and a
  /// connection.
  void _reapClosedSessions(SessionState state) {
    final live = {for (final s in state.sessions) s.id};
    for (final id in _knownSessionIds.toList()) {
      if (!live.contains(id)) {
        _chatBlocs.remove(id);
        _knownSessionIds.remove(id);
      }
    }
    _knownSessionIds.addAll(live);
  }

  @override
  Widget build(BuildContext context) {
    final tokens = AppTokens.of(context);

    return BlocConsumer<SessionBloc, SessionState>(
      listenWhen: (previous, current) =>
          previous.sessions.length != current.sessions.length,
      listener: (context, state) => _reapClosedSessions(state),
      builder: (context, state) {
        final active = state.activeSession;

        return Scaffold(
          body: Row(
            children: [
              // Collapsed, the sidebar narrows to a rail that stays in the
              // layout rather than floating, so the transcript can never render
              // underneath the control that reopens it.
              //
              // The contents are laid out at their settled width throughout,
              // and the shrinking box clips them. Letting them reflow to the
              // animating width instead would re-wrap the header on every
              // frame and overflow it while the box is still narrow.
              AnimatedContainer(
                duration: const Duration(milliseconds: 180),
                curve: Curves.easeOut,
                width: _sidebarOpen ? tokens.sidebarWidth : tokens.railWidth,
                child: ClipRect(
                  child: OverflowBox(
                    alignment: Alignment.centerLeft,
                    minWidth: _sidebarOpen ? tokens.sidebarWidth : tokens.railWidth,
                    maxWidth: _sidebarOpen ? tokens.sidebarWidth : tokens.railWidth,
                    child: _sidebarOpen
                        ? Sidebar(onCollapse: _toggleSidebar)
                        : _SidebarRail(onExpand: _toggleSidebar),
                  ),
                ),
              ),
              const VerticalDivider(width: 1),
              Expanded(
                child: active != null
                    ? ChatView(
                        key: ValueKey(active.id),
                        session: active,
                        bloc: _chatBlocs.of(active.id, active.wsClient),
                      )
                    : const _EmptyState(),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// The collapsed sidebar: just the control that reopens it.
class _SidebarRail extends StatelessWidget {
  final VoidCallback onExpand;

  const _SidebarRail({required this.onExpand});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      color: theme.colorScheme.surfaceContainerLow,
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: Column(
        children: [
          IconButton(
            onPressed: onExpand,
            tooltip: 'Show sessions',
            icon: const Icon(Icons.view_sidebar_outlined, size: 18),
          ),
        ],
      ),
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text('Agent Code', style: theme.textTheme.headlineSmall),
          const SizedBox(height: 8),
          Text(
            'Create a new session to get started',
            style: theme.textTheme.bodyMedium?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
        ],
      ),
    );
  }
}
