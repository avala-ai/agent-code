import 'dart:convert';

import 'package:agent_code_client/models/agent_instance.dart';
import 'package:test/test.dart';

void main() {
  group('AgentInstance', () {
    test('constructor sets all fields', () {
      const instance = AgentInstance(
        pid: 123,
        port: 4096,
        cwd: '/tmp/project',
        token: 'abc-token',
        sessionId: 'sess-1',
      );

      expect(instance.pid, 123);
      expect(instance.port, 4096);
      expect(instance.cwd, '/tmp/project');
      expect(instance.token, 'abc-token');
      expect(instance.sessionId, 'sess-1');
    });

    test('sessionId defaults to null', () {
      const instance = AgentInstance(
        pid: 1,
        port: 4096,
        cwd: '/',
        token: 't',
      );
      expect(instance.sessionId, isNull);
    });

    test('toString includes pid, port, cwd', () {
      const instance = AgentInstance(
        pid: 42,
        port: 8080,
        cwd: '/home/user',
        token: 'x',
      );
      final s = instance.toString();
      expect(s, contains('42'));
      expect(s, contains('8080'));
      expect(s, contains('/home/user'));
    });

    group('fromJson', () {
      test('parses complete JSON', () {
        final json = jsonEncode({
          'pid': 999,
          'port': 5000,
          'cwd': '/projects/myapp',
          'token': 'secret-token-123',
          'session_id': 'sess-abc',
        });

        final instance = AgentInstance.fromJson(json);
        expect(instance.pid, 999);
        expect(instance.port, 5000);
        expect(instance.cwd, '/projects/myapp');
        expect(instance.token, 'secret-token-123');
        expect(instance.sessionId, 'sess-abc');
      });

      test('handles missing token field', () {
        final json = jsonEncode({
          'pid': 1,
          'port': 4096,
          'cwd': '/tmp',
        });

        final instance = AgentInstance.fromJson(json);
        expect(instance.token, '');
      });

      test('handles missing session_id', () {
        final json = jsonEncode({
          'pid': 1,
          'port': 4096,
          'cwd': '/tmp',
          'token': 'tok',
        });

        final instance = AgentInstance.fromJson(json);
        expect(instance.sessionId, isNull);
      });

      test('throws on invalid JSON', () {
        expect(
          () => AgentInstance.fromJson('not json'),
          throwsA(anything),
        );
      });

      test('throws on missing required fields', () {
        expect(
          () => AgentInstance.fromJson('{"pid": 1}'),
          throwsA(anything),
        );
      });
    });
  });
}
