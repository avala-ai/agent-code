import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_desktop/bloc/chat_state.dart';

void main() {
  group('ChatState', () {
    test('defaults to empty state', () {
      const state = ChatState();
      expect(state.messages, isEmpty);
      expect(state.streaming, isFalse);
      expect(state.status, isNull);
      expect(state.pendingPermission, isNull);
      expect(state.error, isNull);
    });

    test('currentAssistantMessage returns null when not streaming', () {
      final state = ChatState(
        messages: [ChatMessage.assistant()],
        streaming: false,
      );
      expect(state.currentAssistantMessage, isNull);
    });

    test('currentAssistantMessage returns last message when streaming', () {
      final msg = ChatMessage.assistant();
      final state = ChatState(
        messages: [ChatMessage.user('hi'), msg],
        streaming: true,
      );
      expect(state.currentAssistantMessage, msg);
    });

    test('currentAssistantMessage returns null if last is user', () {
      final state = ChatState(
        messages: [ChatMessage.user('hi')],
        streaming: true,
      );
      expect(state.currentAssistantMessage, isNull);
    });

    test('currentAssistantMessage returns null when messages empty', () {
      const state = ChatState(streaming: true);
      expect(state.currentAssistantMessage, isNull);
    });

    test('copyWith preserves values', () {
      final state = ChatState(
        messages: [ChatMessage.user('hi')],
        streaming: true,
        error: 'oops',
      );
      final copy = state.copyWith();
      expect(copy.messages.length, 1);
      expect(copy.streaming, isTrue);
      expect(copy.error, 'oops');
    });

    test('copyWith overrides values', () {
      const state = ChatState(streaming: false);
      final copy = state.copyWith(streaming: true);
      expect(copy.streaming, isTrue);
    });

    test('copyWith clearPermission', () {
      final state = ChatState(
        pendingPermission: PermissionRequest(
          requestId: 1,
          toolName: 'Bash',
          inputPreview: 'ls',
        ),
      );
      final copy = state.copyWith(clearPermission: true);
      expect(copy.pendingPermission, isNull);
    });

    test('copyWith clearError', () {
      const state = ChatState(error: 'fail');
      final copy = state.copyWith(clearError: true);
      expect(copy.error, isNull);
    });
  });

  group('PermissionRequest', () {
    test('stores all fields', () {
      const pr = PermissionRequest(
        requestId: 42,
        toolName: 'Bash',
        inputPreview: 'rm -rf /',
      );
      expect(pr.requestId, 42);
      expect(pr.toolName, 'Bash');
      expect(pr.inputPreview, 'rm -rf /');
    });
  });
}
