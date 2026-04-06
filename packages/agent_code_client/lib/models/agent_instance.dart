import 'dart:convert';
import 'dart:io';

/// Represents a running agent process.
class AgentInstance {
  final int pid;
  final int port;
  final String cwd;
  final String token;
  final String? sessionId;

  const AgentInstance({
    required this.pid,
    required this.port,
    required this.cwd,
    required this.token,
    this.sessionId,
  });

  factory AgentInstance.fromLockFile(String path) {
    final content = File(path).readAsStringSync();
    final json = jsonDecode(content) as Map<String, dynamic>;
    return AgentInstance(
      pid: json['pid'] as int,
      port: json['port'] as int,
      cwd: json['cwd'] as String,
      token: json['token'] as String? ?? '',
      sessionId: json['session_id'] as String?,
    );
  }

  @override
  String toString() => 'AgentInstance(pid: $pid, port: $port, cwd: $cwd)';
}
