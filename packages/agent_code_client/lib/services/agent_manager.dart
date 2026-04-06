import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;

import '../models/agent_instance.dart';

/// Manages agent process lifecycle: spawn, health check, kill, discover.
///
///   ┌──────────┐   spawn    ┌───────────┐  health OK  ┌──────────┐
///   │  idle    │──────────►│  starting  │───────────►│  ready   │
///   └──────────┘           └───────────┘             └──────────┘
///                               │ crash/timeout            │ kill
///                               ▼                          ▼
///                          ┌──────────┐              ┌──────────┐
///                          │  failed  │              │  stopped │
///                          └──────────┘              └──────────┘
class AgentManager {
  final Map<int, Process> _processes = {};
  final Map<int, AgentInstance> _instances = {};

  /// Well-known locations for the agent binary.
  static const _binaryPaths = [
    // Bundled inside .app (macOS)
    // Caller should prepend the app bundle Resources path
    '.cargo/bin/agent',
    '/usr/local/bin/agent',
    '/opt/homebrew/bin/agent',
  ];

  /// Find the agent binary path.
  /// [bundledPath] is the path inside the .app bundle, checked first.
  String? findBinary({String? bundledPath}) {
    if (bundledPath != null && File(bundledPath).existsSync()) {
      return bundledPath;
    }

    final home = Platform.environment['HOME'] ?? '';
    for (final relative in _binaryPaths) {
      final path = relative.startsWith('/') ? relative : '$home/$relative';
      if (File(path).existsSync()) {
        return path;
      }
    }
    return null;
  }

  /// Spawn an agent process for the given working directory.
  ///
  /// The agent binds to port 0 (OS-assigned), generates an auth token,
  /// and writes both to a lockfile. We read the lockfile to get the port
  /// and token, then health check before returning.
  Future<AgentInstance> spawn(String cwd, {String? bundledPath}) async {
    final binary = findBinary(bundledPath: bundledPath);
    if (binary == null) {
      throw AgentManagerException(
        'Agent binary not found. Install via: cargo install agent-code',
      );
    }

    final process = await Process.start(
      binary,
      ['serve', '--port', '0', '-C', cwd],
      mode: ProcessStartMode.normal,
    );

    final pid = process.pid;

    // Race between: health check success vs process exit (crash).
    final exitCompleter = Completer<int>();
    process.exitCode.then((code) {
      if (!exitCompleter.isCompleted) exitCompleter.complete(code);
    });

    // Wait for the lockfile to appear with port info.
    final lockfilePath = _lockfilePath(pid);
    AgentInstance? instance;

    for (var i = 0; i < 50; i++) {
      await Future.delayed(const Duration(milliseconds: 100));

      // Check if process died.
      if (exitCompleter.isCompleted) {
        final code = await exitCompleter.future;
        throw AgentManagerException(
          'Agent process exited immediately with code $code',
        );
      }

      // Try to read the lockfile.
      if (File(lockfilePath).existsSync()) {
        try {
          instance = AgentInstance.fromJson(File(lockfilePath).readAsStringSync());
          break;
        } catch (_) {
          // Lockfile not fully written yet, retry.
        }
      }
    }

    if (instance == null) {
      process.kill();
      await process.exitCode;
      throw AgentManagerException(
        'Agent lockfile not created within 5 seconds',
      );
    }

    // Health check the HTTP endpoint.
    final client = http.Client();
    try {
      var healthy = false;
      for (var i = 0; i < 30; i++) {
        await Future.delayed(const Duration(milliseconds: 100));
        try {
          final resp = await client
              .get(Uri.parse('http://127.0.0.1:${instance.port}/health'))
              .timeout(const Duration(seconds: 2));
          if (resp.statusCode == 200) {
            healthy = true;
            break;
          }
        } catch (_) {
          // Not ready yet.
        }
      }

      if (!healthy) {
        process.kill();
        await process.exitCode;
        throw AgentManagerException(
          'Agent health check failed after 3 seconds',
        );
      }
    } finally {
      client.close();
    }

    _processes[pid] = process;
    _instances[pid] = instance;
    return instance;
  }

  /// Kill a managed agent process.
  Future<void> kill(int pid) async {
    final process = _processes.remove(pid);
    _instances.remove(pid);
    if (process != null) {
      process.kill();
      await process.exitCode;
    }
    // Clean up lockfile.
    final lockfile = File(_lockfilePath(pid));
    if (lockfile.existsSync()) {
      lockfile.deleteSync();
    }
  }

  /// Kill all managed agent processes. Call on app shutdown.
  Future<void> killAll() async {
    final pids = _processes.keys.toList();
    await Future.wait(pids.map(kill));
  }

  /// Get all currently managed instances.
  List<AgentInstance> get instances => _instances.values.toList();

  /// Discover running agent instances via bridge lockfiles.
  /// Returns instances NOT already managed by this AgentManager.
  List<AgentInstance> discoverRunning() {
    final bridgeDir = _bridgeDirectory();
    if (bridgeDir == null || !Directory(bridgeDir).existsSync()) {
      return [];
    }

    final managedPids = _instances.keys.toSet();
    final found = <AgentInstance>[];

    for (final entry in Directory(bridgeDir).listSync()) {
      if (entry is! File || !entry.path.endsWith('.lock')) continue;

      try {
        final instance = AgentInstance.fromJson(
            File(entry.path).readAsStringSync());

        // Skip already-managed.
        if (managedPids.contains(instance.pid)) continue;

        // Validate port.
        if (instance.port < 1024) continue;

        // Check process alive (POSIX signal 0).
        final result = Process.runSync('kill', ['-0', '${instance.pid}']);
        if (result.exitCode != 0) {
          // Stale lockfile, clean up.
          entry.deleteSync();
          continue;
        }

        found.add(instance);
      } catch (_) {
        // Malformed lockfile, skip.
      }
    }

    return found;
  }

  String _lockfilePath(int pid) {
    final dir = _bridgeDirectory();
    return '$dir/$pid.lock';
  }

  static String? _bridgeDirectory() {
    if (Platform.isMacOS) {
      final home = Platform.environment['HOME'];
      if (home != null) return '$home/.cache/agent-code/bridge';
    } else if (Platform.isLinux) {
      final xdg = Platform.environment['XDG_CACHE_HOME'];
      final home = Platform.environment['HOME'];
      final base = xdg ?? (home != null ? '$home/.cache' : null);
      if (base != null) return '$base/agent-code/bridge';
    }
    return null;
  }
}

class AgentManagerException implements Exception {
  final String message;
  const AgentManagerException(this.message);

  @override
  String toString() => 'AgentManagerException: $message';
}
