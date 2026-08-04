import 'dart:async';

import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import 'chat_event.dart';
import 'chat_state.dart';

class ChatBloc extends Bloc<ChatEvent, ChatState> {
  final WsClient wsClient;
  StreamSubscription? _notificationSub;
  StreamSubscription? _requestSub;

  ChatBloc({required this.wsClient}) : super(const ChatState()) {
    on<SendMessageRequested>(_onSendMessage);
    on<NotificationReceived>(_onNotification);
    on<PermissionRequestReceived>(_onPermissionRequest);
    on<PermissionResponded>(_onPermissionResponded);
    on<ConnectionLost>(_onConnectionLost);
    on<StatusRefreshed>(_onStatusRefreshed);

    // Subscribe to WebSocket streams.
    _notificationSub = wsClient.notifications.listen(
      (n) => add(NotificationReceived(n)),
    );
    _requestSub = wsClient.incomingRequests.listen(
      (r) => add(PermissionRequestReceived(r)),
    );
  }

  Future<void> _onSendMessage(
    SendMessageRequested event,
    Emitter<ChatState> emit,
  ) async {
    final userMsg = ChatMessage.user(event.content);
    final assistantMsg = ChatMessage.assistant();
    emit(state.copyWith(
      messages: [...state.messages, userMsg, assistantMsg],
      streaming: true,
      clearError: true,
      bumpRevision: true,
    ));

    try {
      // POST message. Events arrive via the notification stream.
      final response = await wsClient.sendMessage(event.content);
      if (response.error != null) {
        _appendToLastAssistant(emit, '\n\n**Error:** ${response.error!.message}');
      }
    } catch (e) {
      _appendToLastAssistant(emit, '\n\n**Error:** $e');
    }
  }

  void _onNotification(
    NotificationReceived event,
    Emitter<ChatState> emit,
  ) {
    final method = event.notification.method;
    final params = event.notification.params;

    switch (method) {
      case 'events/text_delta':
        _updateLastAssistant(
          emit,
          (m) => m.appendContent(params['text'] as String? ?? ''),
        );
        break;

      case 'events/thinking':
        _updateLastAssistant(
          emit,
          (m) => m.appendThinking(params['text'] as String? ?? ''),
        );
        break;

      case 'events/tool_start':
        final input = params['input'];
        _updateLastAssistant(
          emit,
          (m) => m.addToolCall(ToolCall(
            name: params['name'] as String? ?? 'unknown',
            input: input is Map ? Map<String, dynamic>.from(input) : null,
          )),
        );
        break;

      case 'events/tool_result':
        final name = params['name'] as String? ?? '';
        final isError = params['is_error'] as bool? ?? false;
        final content = params['content'] as String?;
        _updateLastAssistant(emit, (m) {
          // Resolve the most recent still-running call with this name. The
          // server sends no call id, so identity is name plus running state.
          for (var i = m.toolCalls.length - 1; i >= 0; i--) {
            final call = m.toolCalls[i];
            if (call.name == name && call.isRunning) {
              return m.replaceToolCall(
                i,
                call.copyWith(
                  status: isError ? ToolCallStatus.error : ToolCallStatus.done,
                  result: content,
                ),
              );
            }
          }
          return m;
        });
        break;

      case 'events/done':
        emit(state.copyWith(streaming: false));
        unawaited(_refreshStatus());
        break;

      case 'events/error':
        _appendToLastAssistant(
          emit,
          '\n\n**Error:** ${params['message'] as String? ?? 'Unknown error'}',
        );
        break;

      case 'events/usage':
      case 'events/turn_complete':
      case 'events/warning':
      case 'events/compact':
        // Informational, no UI update needed.
        break;
    }
  }

  void _onPermissionRequest(
    PermissionRequestReceived event,
    Emitter<ChatState> emit,
  ) {
    final params = event.request.params;
    emit(state.copyWith(
      pendingPermission: PermissionRequest(
        requestId: event.request.id,
        toolName: params['tool'] as String? ?? 'Unknown',
        inputPreview: params['input'] as String? ?? '',
      ),
    ));
  }

  void _onPermissionResponded(
    PermissionResponded event,
    Emitter<ChatState> emit,
  ) {
    wsClient.respondPermission(event.requestId, event.decision);
    emit(state.copyWith(clearPermission: true));
  }

  void _onConnectionLost(
    ConnectionLost event,
    Emitter<ChatState> emit,
  ) {
    emit(state.copyWith(
      streaming: false,
      error: 'Connection to agent lost',
    ));
  }

  void _onStatusRefreshed(
    StatusRefreshed event,
    Emitter<ChatState> emit,
  ) {
    emit(state.copyWith(status: event.status));
  }

  /// Applies [update] to the trailing assistant message, creating one first if
  /// the transcript does not currently end with one, and emits the result.
  void _updateLastAssistant(
    Emitter<ChatState> emit,
    ChatMessage Function(ChatMessage) update,
  ) {
    final messages = List<ChatMessage>.of(state.messages);
    final grew = messages.isEmpty || !messages.last.isAssistant;
    if (grew) messages.add(ChatMessage.assistant());

    final last = messages.last;
    final updated = update(last);
    if (!grew && identical(updated, last)) return;

    messages[messages.length - 1] = updated;
    emit(state.copyWith(
      messages: messages,
      streaming: grew ? true : null,
      bumpRevision: true,
    ));
  }

  void _appendToLastAssistant(Emitter<ChatState> emit, String text) =>
      _updateLastAssistant(emit, (m) => m.appendContent(text));

  Future<void> _refreshStatus() async {
    try {
      final status = await wsClient.getStatus();
      if (!isClosed) add(StatusRefreshed(status));
    } catch (_) {
      // Best-effort.
    }
  }

  @override
  Future<void> close() async {
    await _notificationSub?.cancel();
    await _requestSub?.cancel();
    return super.close();
  }
}
