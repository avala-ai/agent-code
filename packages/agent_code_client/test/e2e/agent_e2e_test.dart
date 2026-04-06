@Tags(['e2e'])
import 'dart:async';
import 'dart:io';

import 'package:agent_code_client/models/agent_instance.dart';
import 'package:agent_code_client/models/json_rpc.dart';
import 'package:agent_code_client/services/ws_client.dart';
import 'package:http/http.dart' as http;
import 'package:test/test.dart';

/// E2E tests that spawn a real agent binary, connect via WebSocket,
/// and send prompts through OpenRouter to verify the full pipeline.
///
/// Requirements:
///   - `agent` binary built and in PATH (or set AGENT_BINARY_PATH)
///   - OPENROUTER_API_KEY env var set
///
/// Run: dart test --tags e2e test/e2e/
/// Skip: dart test --exclude-tags e2e (default)
void main() {
  final apiKey = Platform.environment['OPENROUTER_API_KEY'];
  final agentBinary = _findAgentBinary();

  group('Agent E2E', () {
    Process? agentProcess;
    AgentInstance? instance;
    WsClient? ws;

    setUp(() async {
      if (apiKey == null || apiKey.isEmpty || agentBinary == null) return;
      // Type system narrows after the null check above.
      final binary = agentBinary;
      final key = apiKey;

      // Create a temp directory for the agent to work in.
      final workDir = await Directory.systemTemp.createTemp('agent_e2e_');

      // Spawn the agent with OpenRouter.
      agentProcess = await Process.start(
        binary,
        ['serve', '--port', '0', '-C', workDir.path],
        environment: {
          'OPENROUTER_API_KEY': key,
          'AGENT_CODE_PROVIDER': 'openrouter',
          'AGENT_CODE_MODEL': 'meta-llama/llama-3.3-70b-instruct',
          ...Platform.environment,
        },
      );

      // Wait for lockfile.
      instance = await _waitForLockfile(agentProcess!.pid);

      // Connect WebSocket.
      ws = WsClient();
      await ws!.connect(instance!.port, instance!.token);
    });

    tearDown(() async {
      if (apiKey == null || agentBinary == null) return;
      await ws?.dispose();
      if (agentProcess != null) {
        agentProcess!.kill();
        await agentProcess!.exitCode;
        final lockfile = _lockfilePath(agentProcess!.pid);
        if (File(lockfile).existsSync()) File(lockfile).deleteSync();
      }
    });

    test(
      'health endpoint responds',
      () async {
        if (apiKey == null || agentBinary == null) {
          markTestSkipped('OPENROUTER_API_KEY not set or agent binary not found');
          return;
        }

        final client = http.Client();
        try {
          final resp = await client.get(
            Uri.parse('http://127.0.0.1:${instance!.port}/health'),
          );
          expect(resp.statusCode, 200);
          expect(resp.body, 'ok');
        } finally {
          client.close();
        }
      },
      timeout: const Timeout(Duration(seconds: 15)),
    );

    test(
      'status returns session info',
      () async {
        if (apiKey == null || agentBinary == null) {
          markTestSkipped('OPENROUTER_API_KEY not set or agent binary not found');
          return;
        }

        final status = await ws!.getStatus();
        expect(status.sessionId, isNotEmpty);
        expect(status.version, isNotEmpty);
        expect(status.turnCount, 0);
        expect(status.costUsd, 0.0);
      },
      timeout: const Timeout(Duration(seconds: 15)),
    );

    test(
      'send message and receive streaming events',
      () async {
        if (apiKey == null || agentBinary == null) {
          markTestSkipped('OPENROUTER_API_KEY not set or agent binary not found');
          return;
        }

        // Collect events.
        final events = <JsonRpcNotification>[];
        final doneFuture = Completer<void>();

        final sub = ws!.notifications.listen((event) {
          events.add(event);
          if (event.method == 'events/done') {
            doneFuture.complete();
          }
        });

        // Send a simple prompt that doesn't require tools.
        final response = await ws!.sendMessage('Say exactly: hello world');

        // Wait for the done event (or timeout).
        await doneFuture.future.timeout(
          const Duration(seconds: 30),
          onTimeout: () {
            // May have already received done via the response.
          },
        );

        await sub.cancel();

        // Verify response.
        expect(response.error, isNull);
        expect(response.result, isNotNull);
        expect(response.result!['response'], isNotEmpty);
        expect(response.result!['turn_count'], greaterThan(0));

        // Verify we received streaming events.
        final textDeltas =
            events.where((e) => e.method == 'events/text_delta').toList();
        expect(textDeltas, isNotEmpty, reason: 'Should receive text_delta events');

        final doneEvents =
            events.where((e) => e.method == 'events/done').toList();
        expect(doneEvents, hasLength(1), reason: 'Should receive exactly one done event');
      },
      timeout: const Timeout(Duration(seconds: 60)),
    );

    test(
      'send message that triggers a tool call',
      () async {
        if (apiKey == null || agentBinary == null) {
          markTestSkipped('OPENROUTER_API_KEY not set or agent binary not found');
          return;
        }

        final events = <JsonRpcNotification>[];
        final doneFuture = Completer<void>();

        final sub = ws!.notifications.listen((event) {
          events.add(event);
          if (event.method == 'events/done') {
            if (!doneFuture.isCompleted) doneFuture.complete();
          }
        });

        // Ask something that will use the Bash tool.
        final response = await ws!.sendMessage('What is the current date? Use the bash tool to run "date".');

        await doneFuture.future.timeout(
          const Duration(seconds: 45),
          onTimeout: () {},
        );

        await sub.cancel();

        // Should have tool events.
        final toolStarts =
            events.where((e) => e.method == 'events/tool_start').toList();
        final toolResults =
            events.where((e) => e.method == 'events/tool_result').toList();

        expect(toolStarts, isNotEmpty,
            reason: 'Should use at least one tool');
        expect(toolResults, isNotEmpty,
            reason: 'Tools should produce results');

        // Response should contain the date.
        expect(response.result, isNotNull);
      },
      timeout: const Timeout(Duration(seconds: 90)),
    );

    test(
      'multiple turns maintain conversation context',
      () async {
        if (apiKey == null || agentBinary == null) {
          markTestSkipped('OPENROUTER_API_KEY not set or agent binary not found');
          return;
        }

        // Turn 1: establish a fact.
        await ws!.sendMessage('Remember: the secret code is ALPHA-7.');

        // Turn 2: ask about it.
        final response = await ws!.sendMessage('What is the secret code I told you?');

        expect(response.result, isNotNull);
        final text = response.result!['response'] as String;
        expect(text.toLowerCase(), contains('alpha'));
        expect(response.result!['turn_count'], 2);
      },
      timeout: const Timeout(Duration(seconds: 90)),
    );

    test(
      'status updates after a turn',
      () async {
        if (apiKey == null || agentBinary == null) {
          markTestSkipped('OPENROUTER_API_KEY not set or agent binary not found');
          return;
        }

        await ws!.sendMessage('Say hi.');

        final status = await ws!.getStatus();
        expect(status.turnCount, 1);
        expect(status.costUsd, greaterThan(0));
        expect(status.messageCount, greaterThan(0));
      },
      timeout: const Timeout(Duration(seconds: 60)),
    );
  });
}

/// Find the agent binary.
String? _findAgentBinary() {
  // Check env var first.
  final envPath = Platform.environment['AGENT_BINARY_PATH'];
  if (envPath != null && File(envPath).existsSync()) return envPath;

  // Check common locations.
  final candidates = [
    '${Platform.environment['HOME']}/.cargo/bin/agent',
    '/usr/local/bin/agent',
    '/opt/homebrew/bin/agent',
    // Relative to repo root (for CI after cargo build).
    'target/release/agent',
    'target/debug/agent',
  ];

  for (final path in candidates) {
    if (File(path).existsSync()) return path;
  }

  // Check PATH via `which`.
  try {
    final result = Process.runSync('which', ['agent']);
    if (result.exitCode == 0) {
      return result.stdout.toString().trim();
    }
  } catch (_) {}

  return null;
}

/// Wait for the agent's lockfile to appear and parse it.
Future<AgentInstance> _waitForLockfile(int pid) async {
  final path = _lockfilePath(pid);

  for (var i = 0; i < 100; i++) {
    await Future.delayed(const Duration(milliseconds: 100));
    final file = File(path);
    if (file.existsSync()) {
      try {
        return AgentInstance.fromJson(file.readAsStringSync());
      } catch (_) {
        // Not fully written yet.
      }
    }
  }

  throw StateError('Agent lockfile not created within 10 seconds');
}

String _lockfilePath(int pid) {
  final home = Platform.environment['HOME'] ?? '';
  return '$home/.cache/agent-code/bridge/$pid.lock';
}
