import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mocktail/mocktail.dart';

import 'package:agent_code_client_app/bloc/chat_bloc_registry.dart';
import 'package:agent_code_client_app/bloc/chat_event.dart';

class MockWsClient extends Mock implements WsClient {}

MockWsClient _ws() {
  final ws = MockWsClient();
  when(() => ws.notifications).thenAnswer((_) => const Stream.empty());
  when(() => ws.incomingRequests).thenAnswer((_) => const Stream.empty());
  return ws;
}

void main() {
  group('ChatBlocRegistry', () {
    test('returns the same bloc for a session across lookups', () {
      final registry = ChatBlocRegistry();
      final ws = _ws();
      expect(identical(registry.of('s1', ws), registry.of('s1', ws)), isTrue);
      addTearDown(registry.clear);
    });

    test('separate sessions get separate blocs', () {
      final registry = ChatBlocRegistry();
      expect(registry.of('s1', _ws()), isNot(same(registry.of('s2', _ws()))));
      expect(registry.length, 2);
      addTearDown(registry.clear);
    });

    test('a transcript survives switching away and back', () {
      // The regression this exists to prevent: a bloc owned by the view is
      // closed on unmount, so returning to a session shows an empty chat.
      final registry = ChatBlocRegistry();
      final ws = _ws();

      final bloc = registry.of('s1', ws);
      bloc.add(NotificationReceived(const JsonRpcNotification(
        method: 'events/text_delta',
        params: {'text': 'partial reply'},
      )));

      return Future<void>.delayed(Duration.zero, () {
        // Simulate switching to another session and back.
        registry.of('s2', _ws());
        final returned = registry.of('s1', ws);

        expect(returned.state.messages, isNotEmpty);
        expect(returned.state.messages.last.content, 'partial reply');
        expect(returned.isClosed, isFalse);
        addTearDown(registry.clear);
      });
    });

    test('peek does not create a bloc', () {
      final registry = ChatBlocRegistry();
      expect(registry.peek('s1'), isNull);
      expect(registry.length, 0);
    });

    test('remove closes the bloc and forgets the key', () async {
      final registry = ChatBlocRegistry();
      final bloc = registry.of('s1', _ws());

      await registry.remove('s1');

      expect(bloc.isClosed, isTrue);
      expect(registry.has('s1'), isFalse);
      expect(registry.length, 0);
    });

    test('removing an unknown session is harmless', () async {
      final registry = ChatBlocRegistry();
      await registry.remove('ghost');
      expect(registry.length, 0);
    });

    test('clear closes every bloc', () async {
      final registry = ChatBlocRegistry();
      final a = registry.of('s1', _ws());
      final b = registry.of('s2', _ws());

      await registry.clear();

      expect(a.isClosed, isTrue);
      expect(b.isClosed, isTrue);
      expect(registry.length, 0);
    });

    test('a session reopened after removal starts clean', () async {
      final registry = ChatBlocRegistry();
      final ws = _ws();
      final first = registry.of('s1', ws);
      await registry.remove('s1');

      final second = registry.of('s1', ws);
      expect(second, isNot(same(first)));
      expect(second.state.messages, isEmpty);
      addTearDown(registry.clear);
    });
  });
}
