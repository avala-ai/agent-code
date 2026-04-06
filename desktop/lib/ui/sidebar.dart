import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import '../bloc/session_bloc.dart';
import '../bloc/session_event.dart';
import '../bloc/session_state.dart';

class Sidebar extends StatelessWidget {
  const Sidebar({super.key});

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
                  ],
                ),
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

  const _SessionTile({required this.session, required this.isActive});

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
              Expanded(
                child: Text(
                  folderName,
                  style: theme.textTheme.bodyMedium?.copyWith(
                    color: isActive
                        ? theme.colorScheme.onPrimaryContainer
                        : theme.colorScheme.onSurface,
                  ),
                  overflow: TextOverflow.ellipsis,
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
