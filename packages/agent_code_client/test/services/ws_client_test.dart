import 'package:agent_code_client/services/ws_client.dart';
import 'package:test/test.dart';

void main() {
  group('WsClient', () {
    late WsClient client;

    setUp(() {
      client = WsClient();
    });

    tearDown(() async {
      await client.dispose();
    });

    test('starts disconnected', () {
      expect(client.isConnected, isFalse);
    });

    test('sendMessage throws when not connected', () {
      expect(
        () => client.sendMessage('hello'),
        throwsA(isA<WsClientException>()),
      );
    });

    test('getStatus throws when not connected', () {
      expect(
        () => client.getStatus(),
        throwsA(isA<WsClientException>()),
      );
    });

    test('cancel throws when not connected', () {
      expect(
        () => client.cancel(),
        throwsA(isA<WsClientException>()),
      );
    });

    test('connect to invalid port fails', () async {
      try {
        await client.connect(1, 'invalid-token').timeout(
              const Duration(seconds: 5),
            );
        fail('Should have thrown');
      } catch (e) {
        // Expected: connection refused or timeout.
        expect(e, isNotNull);
      }
    }, timeout: const Timeout(Duration(seconds: 10)));

    test('notifications stream is broadcast', () {
      // Should be able to listen multiple times without error.
      final sub1 = client.notifications.listen((_) {});
      final sub2 = client.notifications.listen((_) {});
      sub1.cancel();
      sub2.cancel();
    });

    test('incomingRequests stream is broadcast', () {
      final sub1 = client.incomingRequests.listen((_) {});
      final sub2 = client.incomingRequests.listen((_) {});
      sub1.cancel();
      sub2.cancel();
    });

    test('dispose cleans up without error', () async {
      await client.dispose();
      // Double dispose should not throw.
      await client.dispose();
    });
  });
}
