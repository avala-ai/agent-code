import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import '../bloc/session_activity.dart';
import '../bloc/session_bloc.dart';
import '../bloc/session_event.dart';
import '../bloc/session_state.dart';

/// Map a [SessionActivity] to a theme-derived color. Needs-input and working
/// are prominent; idle/done are muted; failed uses the error color.
Color activityColor(ThemeData theme, SessionActivity activity) {
  final scheme = theme.colorScheme;
  return switch (activity) {
    SessionActivity.needsInput => scheme.tertiary,
    SessionActivity.working => scheme.primary,
    SessionActivity.idle => scheme.onSurfaceVariant,
    SessionActivity.done => scheme.secondary,
    SessionActivity.failed => scheme.error,
  };
}

class Sidebar extends StatelessWidget {
  /// Collapses the sidebar to its rail. Omitted where the sidebar cannot be
  /// collapsed, in which case no control is shown.
  final VoidCallback? onCollapse;

  const Sidebar({super.key, this.onCollapse});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return BlocBuilder<SessionBloc, SessionState>(
      builder: (context, state) {
        return Container(
          color: theme.colorScheme.surfaceContainerLow,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.all(12),
                child: Row(
                  children: [
                    Text(
                      'SESSIONS',
                      style: theme.textTheme.labelSmall?.copyWith(
                        letterSpacing: 0.5,
                        color: theme.colorScheme.onSurfaceVariant,
                      ),
                    ),
                    const Spacer(),
                    _NewSessionButton(),
                    if (onCollapse != null)
                      IconButton(
                        onPressed: onCollapse,
                        tooltip: 'Hide sessions',
                        visualDensity: VisualDensity.compact,
                        icon: const Icon(Icons.view_sidebar_outlined, size: 18),
                      ),
                  ],
                ),
              ),
              if (state.sessions.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(
                      left: 12, right: 12, bottom: 8),
                  child: _ActivitySummary(state: state),
                ),
              Expanded(
                child: state.sessions.isEmpty
                    ? Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 12),
                        child: Text(
                          'No sessions yet.\nClick + to start.',
                          style: theme.textTheme.bodySmall?.copyWith(
                            color: theme.colorScheme.onSurfaceVariant,
                          ),
                        ),
                      )
                    : ListView.builder(
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        itemCount: state.sessions.length,
                        itemBuilder: (context, index) {
                          final session = state.sessions[index];
                          final isActive = session.id == state.activeSessionId;
                          return _SessionTile(
                            session: session,
                            isActive: isActive,
                            activity: state.activityFor(session.id),
                          );
                        },
                      ),
              ),
              if (state.error != null)
                Padding(
                  padding: const EdgeInsets.all(12),
                  child: Text(
                    state.error!,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: theme.colorScheme.error,
                    ),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
            ],
          ),
        );
      },
    );
  }
}

/// One-line summary above the list: `N working · M need input · K idle`.
/// Zero counts are omitted; when nothing is active it reads "All idle".
class _ActivitySummary extends StatelessWidget {
  final SessionState state;

  const _ActivitySummary({required this.state});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    var working = 0;
    var needsInput = 0;
    var idle = 0;
    var done = 0;
    var failed = 0;
    for (final session in state.sessions) {
      final a = state.activityFor(session.id);
      if (a == SessionActivity.working) {
        working++;
      } else if (a == SessionActivity.needsInput) {
        needsInput++;
      } else if (a == SessionActivity.idle) {
        idle++;
      } else if (a == SessionActivity.done) {
        done++;
      } else if (a == SessionActivity.failed) {
        failed++;
      }
    }

    final parts = <String>[
      if (needsInput > 0) '$needsInput need input',
      if (working > 0) '$working working',
      if (idle > 0) '$idle idle',
      if (done > 0) '$done done',
      if (failed > 0) '$failed failed',
    ];

    // Calm state: everything that exists is idle (or nothing but idle counts).
    final text = (working == 0 && needsInput == 0 && done == 0 && failed == 0)
        ? 'All idle'
        : parts.join(' · ');

    final prominent = needsInput > 0 || working > 0 || failed > 0;

    return Text(
      text,
      key: const Key('activity-summary'),
      style: theme.textTheme.labelMedium?.copyWith(
        color: prominent
            ? theme.colorScheme.onSurface
            : theme.colorScheme.onSurfaceVariant,
        fontWeight: prominent ? FontWeight.w600 : FontWeight.w400,
      ),
    );
  }
}

class _NewSessionButton extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 28,
      child: TextButton(
        onPressed: () => _pickFolderAndCreate(context),
        child: const Text('+ New'),
      ),
    );
  }

  Future<void> _pickFolderAndCreate(BuildContext context) async {
    // For now, use the current directory. macOS folder picker requires
    // file_selector_macos package or platform channel.
    final cwd = Directory.current.path;
    context.read<SessionBloc>().add(CreateSessionRequested(cwd));
  }
}

class _SessionTile extends StatelessWidget {
  final SessionData session;
  final bool isActive;
  final SessionActivity activity;

  const _SessionTile({
    required this.session,
    required this.isActive,
    required this.activity,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final folderName = _folderName(session.instance.cwd);

    return Material(
      color: isActive ? theme.colorScheme.primaryContainer : Colors.transparent,
      borderRadius: BorderRadius.circular(8),
      child: InkWell(
        borderRadius: BorderRadius.circular(8),
        onTap: () {
          context.read<SessionBloc>().add(SwitchSessionRequested(session.id));
        },
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
          child: Row(
            children: [
              _StatusDot(activity: activity),
              const SizedBox(width: 8),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      folderName,
                      style: theme.textTheme.bodyMedium?.copyWith(
                        color: isActive
                            ? theme.colorScheme.onPrimaryContainer
                            : theme.colorScheme.onSurface,
                      ),
                      overflow: TextOverflow.ellipsis,
                    ),
                    Text(
                      activity.label,
                      style: theme.textTheme.labelSmall?.copyWith(
                        color: activityColor(theme, activity),
                        fontWeight: activity == SessionActivity.needsInput ||
                                activity == SessionActivity.working
                            ? FontWeight.w600
                            : FontWeight.w400,
                      ),
                    ),
                  ],
                ),
              ),
              _CloseButton(sessionId: session.id, isActive: isActive),
            ],
          ),
        ),
      ),
    );
  }

  static String _folderName(String cwd) {
    final parts = cwd.split(RegExp(r'[/\\]')).where((p) => p.isNotEmpty).toList();
    return parts.isNotEmpty ? parts.last : cwd;
  }
}

/// Colored dot indicating a session's live activity.
class _StatusDot extends StatelessWidget {
  final SessionActivity activity;

  const _StatusDot({required this.activity});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = activityColor(theme, activity);
    return Container(
      width: 8,
      height: 8,
      decoration: BoxDecoration(
        color: color,
        shape: BoxShape.circle,
      ),
    );
  }
}

class _CloseButton extends StatelessWidget {
  final String sessionId;
  final bool isActive;

  const _CloseButton({required this.sessionId, required this.isActive});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 24,
      height: 24,
      child: IconButton(
        padding: EdgeInsets.zero,
        iconSize: 14,
        icon: const Icon(Icons.close),
        onPressed: () {
          context.read<SessionBloc>().add(DestroySessionRequested(sessionId));
        },
      ),
    );
  }
}
