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
    // Add user message.
    final userMsg = ChatMessage.user(event.content);
    final messages = [...state.messages, userMsg];

    // Start assistant message placeholder.
    final assistantMsg = ChatMessage.assistant();
    emit(state.copyWith(
      messages: [...messages, assistantMsg],
      streaming: true,
      clearError: true,
    ));

    try {
      // POST message. Events arrive via notification stream.
      final response = await wsClient.sendMessage(event.content);
      if (response.error != null) {
        _appendToCurrentMessage('\n\n**Error:** ${response.error!.message}');
      }
    } catch (e) {
      _appendToCurrentMessage('\n\n**Error:** $e');
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
        _ensureAssistantMessage(emit);
        final text = params['text'] as String? ?? '';
        _appendToCurrentMessage(text);
        emit(state.copyWith(messages: List.of(state.messages)));
        break;

      case 'events/thinking':
        _ensureAssistantMessage(emit);
        final current = state.messages.last;
        current.thinking = (current.thinking ?? '') + (params['text'] as String? ?? '');
        emit(state.copyWith(messages: List.of(state.messages)));
        break;

      case 'events/tool_start':
        _ensureAssistantMessage(emit);
        final name = params['name'] as String? ?? 'unknown';
        state.messages.last.toolCalls.add(ToolCall(name: name));
        emit(state.copyWith(messages: List.of(state.messages)));
        break;

      case 'events/tool_result':
        final name = params['name'] as String? ?? '';
        final isError = params['is_error'] as bool? ?? false;
        final tools = state.messages.last.toolCalls;
        for (var i = tools.length - 1; i >= 0; i--) {
          if (tools[i].name == name && tools[i].status == ToolCallStatus.running) {
            tools[i].status = isError ? ToolCallStatus.error : ToolCallStatus.done;
            break;
          }
        }
        emit(state.copyWith(messages: List.of(state.messages)));
        break;

      case 'events/done':
        emit(state.copyWith(streaming: false));
        // Refresh status.
        _refreshStatus();
        break;

      case 'events/error':
        _ensureAssistantMessage(emit);
        final message = params['message'] as String? ?? 'Unknown error';
        _appendToCurrentMessage('\n\n**Error:** $message');
        emit(state.copyWith(messages: List.of(state.messages)));
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

  void _ensureAssistantMessage(Emitter<ChatState> emit) {
    if (state.messages.isEmpty || state.messages.last.role != 'assistant') {
      final msg = ChatMessage.assistant();
      emit(state.copyWith(
        messages: [...state.messages, msg],
        streaming: true,
      ));
    }
  }

  void _appendToCurrentMessage(String text) {
    if (state.messages.isNotEmpty && state.messages.last.role == 'assistant') {
      state.messages.last.content += text;
    }
  }

  Future<void> _refreshStatus() async {
    try {
      final status = await wsClient.getStatus();
      // ignore: invalid_use_of_visible_for_testing_member
      emit(state.copyWith(status: status));
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
