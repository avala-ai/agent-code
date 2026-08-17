import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import '../bloc/chat_bloc.dart';
import '../bloc/chat_event.dart';
import '../bloc/chat_state.dart';
import '../bloc/session_state.dart';
import 'app_theme.dart';
import 'composer.dart';
import 'composer_logic.dart';
import 'drafts.dart';
import 'message_bubble.dart';
import 'permission_dialog.dart';
import 'status_bar.dart';

class ChatView extends StatefulWidget {
  final SessionData session;

  /// Chat state for this session. Supplied by the shell so a session's turn
  /// keeps running when its view is not mounted; created locally when absent.
  final ChatBloc? bloc;

  const ChatView({super.key, required this.session, this.bloc});

  @override
  State<ChatView> createState() => _ChatViewState();
}

class _ChatViewState extends State<ChatView> {
  late final ChatBloc _chatBloc;
  late final bool _ownsBloc;
  final _scrollController = ScrollController();

  List<SkillItem> _skills = const [];

  @override
  void initState() {
    super.initState();
    _ownsBloc = widget.bloc == null;
    _chatBloc = widget.bloc ?? ChatBloc(wsClient: widget.session.wsClient);
    _loadSkills();
  }

  @override
  void dispose() {
    if (_ownsBloc) _chatBloc.close();
    _scrollController.dispose();
    super.dispose();
  }

  Future<void> _loadSkills() async {
    try {
      final skills = await widget.session.wsClient.getSkills();
      if (!mounted) return;
      setState(() {
        _skills = [
          for (final s in skills)
            SkillItem(
              name: s.name,
              description: s.description,
              argumentHint: s.argumentHint,
            ),
        ];
      });
    } catch (_) {
      // The picker is an affordance, not a requirement — an agent that cannot
      // list its skills still takes messages.
    }
  }

  String get _draftKey => widget.session.id;

  void _send(String text) {
    Drafts.instance.clear(_draftKey);
    _chatBloc.add(SendMessageRequested(text));
  }

  void _stop() => widget.session.wsClient.cancel();

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scrollController.hasClients) {
        _scrollController.animateTo(
          _scrollController.position.maxScrollExtent,
          duration: const Duration(milliseconds: 200),
          curve: Curves.easeOut,
        );
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    return BlocProvider.value(
      value: _chatBloc,
      child: BlocConsumer<ChatBloc, ChatState>(
        listener: (context, state) => _scrollToBottom(),
        builder: (context, state) {
          return Scaffold(
            body: Column(
              children: [
                Expanded(child: _buildMessageList(state)),
                _buildComposerArea(state),
                StatusBar(status: state.status, streaming: state.streaming),
              ],
            ),
            bottomSheet: state.pendingPermission != null
                ? PermissionDialogWidget(
                    permission: state.pendingPermission!,
                    onRespond: (requestId, decision) {
                      _chatBloc.add(PermissionResponded(requestId, decision));
                    },
                  )
                : null,
          );
        },
      ),
    );
  }

  Widget _buildMessageList(ChatState state) {
    final theme = Theme.of(context);
    final tokens = AppTokens.of(context);

    if (state.messages.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text('Agent Code', style: theme.textTheme.titleLarge),
            const SizedBox(height: 4),
            Text(
              'Send a message to start in ${widget.session.instance.cwd}',
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      );
    }

    return _ReadingColumn(
      maxWidth: tokens.contentWidth,
      child: ListView.builder(
        controller: _scrollController,
        padding: EdgeInsets.symmetric(
          horizontal: tokens.chatPadding,
          vertical: 16,
        ),
        itemCount: state.messages.length,
        itemBuilder: (context, index) {
          final msg = state.messages[index];
          final isStreaming = index == state.messages.length - 1 &&
              msg.isAssistant &&
              state.streaming;
          return Padding(
            padding: const EdgeInsets.only(bottom: 16),
            child: MessageBubble(
              key: ValueKey(msg.id),
              message: msg,
              streaming: isStreaming,
            ),
          );
        },
      ),
    );
  }

  Widget _buildComposerArea(ChatState state) {
    final tokens = AppTokens.of(context);
    return Container(
      padding: EdgeInsets.fromLTRB(tokens.chatPadding, 12, tokens.chatPadding, 16),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: Theme.of(context).dividerColor)),
      ),
      child: _ReadingColumn(
        maxWidth: tokens.contentWidth,
        child: Composer(
          streaming: state.streaming,
          inputBlocked: state.inputBlocked,
          skills: _skills,
          initialDraft: Drafts.instance.read(_draftKey),
          onDraftChanged: (text) => Drafts.instance.write(_draftKey, text),
          onSend: _send,
          onStop: _stop,
        ),
      ),
    );
  }
}

/// Caps content at a comfortable reading width and centres it, so a long reply
/// does not run the full width of a desktop display.
class _ReadingColumn extends StatelessWidget {
  final double maxWidth;
  final Widget child;

  const _ReadingColumn({required this.maxWidth, required this.child});

  @override
  Widget build(BuildContext context) => Center(
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: maxWidth),
          child: child,
        ),
      );
}
