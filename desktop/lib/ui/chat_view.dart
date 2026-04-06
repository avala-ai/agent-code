import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import '../bloc/chat_bloc.dart';
import '../bloc/chat_event.dart';
import '../bloc/chat_state.dart';
import '../bloc/session_state.dart';
import 'message_bubble.dart';
import 'permission_dialog.dart';
import 'status_bar.dart';

class ChatView extends StatefulWidget {
  final SessionData session;

  const ChatView({super.key, required this.session});

  @override
  State<ChatView> createState() => _ChatViewState();
}

class _ChatViewState extends State<ChatView> {
  late final ChatBloc _chatBloc;
  final _controller = TextEditingController();
  final _scrollController = ScrollController();
  final _focusNode = FocusNode();

  @override
  void initState() {
    super.initState();
    _chatBloc = ChatBloc(wsClient: widget.session.wsClient);
  }

  @override
  void dispose() {
    _chatBloc.close();
    _controller.dispose();
    _scrollController.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  void _send() {
    final text = _controller.text.trim();
    if (text.isEmpty) return;
    _controller.clear();
    _chatBloc.add(SendMessageRequested(text));
  }

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
        listener: (context, state) {
          _scrollToBottom();
          // Show permission dialog as overlay.
        },
        builder: (context, state) {
          return Scaffold(
            body: Column(
              children: [
                Expanded(child: _buildMessageList(state)),
                _buildInputArea(state),
                StatusBar(status: state.status, streaming: state.streaming),
              ],
            ),
            // Permission dialog overlay.
            floatingActionButton: state.pendingPermission != null
                ? null // Dialog shown via overlay below
                : null,
            // Use a stack for the permission dialog overlay.
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
    if (state.messages.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              'Agent Code',
              style: Theme.of(context).textTheme.titleLarge,
            ),
            const SizedBox(height: 4),
            Text(
              'Send a message to start in ${widget.session.instance.cwd}',
              style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                    color: Theme.of(context).colorScheme.onSurfaceVariant,
                  ),
            ),
          ],
        ),
      );
    }

    return ListView.builder(
      controller: _scrollController,
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 16),
      itemCount: state.messages.length,
      itemBuilder: (context, index) {
        final msg = state.messages[index];
        final isStreaming = index == state.messages.length - 1 &&
            msg.role == 'assistant' &&
            state.streaming;
        return Padding(
          padding: const EdgeInsets.only(bottom: 16),
          child: MessageBubble(message: msg, streaming: isStreaming),
        );
      },
    );
  }

  Widget _buildInputArea(ChatState state) {
    return Container(
      padding: const EdgeInsets.fromLTRB(24, 12, 24, 16),
      decoration: BoxDecoration(
        border: Border(
          top: BorderSide(color: Theme.of(context).dividerColor),
        ),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Expanded(
            child: KeyboardListener(
              focusNode: _focusNode,
              onKeyEvent: (event) {
                if (event is KeyDownEvent &&
                    event.logicalKey == LogicalKeyboardKey.enter &&
                    HardwareKeyboard.instance.isMetaPressed &&
                    !state.streaming) {
                  _send();
                }
              },
              child: TextField(
                controller: _controller,
                maxLines: null,
                minLines: 1,
                decoration: InputDecoration(
                  hintText: 'Send a message... (\u2318\u21B5 to send)',
                  border: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(12),
                  ),
                  contentPadding: const EdgeInsets.symmetric(
                    horizontal: 14,
                    vertical: 10,
                  ),
                ),
                enabled: !state.streaming,
              ),
            ),
          ),
          const SizedBox(width: 8),
          FilledButton(
            onPressed: state.streaming ? null : _send,
            child: const Text('Send'),
          ),
        ],
      ),
    );
  }
}
