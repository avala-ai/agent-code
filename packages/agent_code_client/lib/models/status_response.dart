/// Response from the agent's status query.
class StatusResponse {
  final String sessionId;
  final String model;
  final String cwd;
  final int turnCount;
  final int messageCount;
  final double costUsd;
  final bool planMode;
  final String version;

  const StatusResponse({
    required this.sessionId,
    required this.model,
    required this.cwd,
    required this.turnCount,
    required this.messageCount,
    required this.costUsd,
    required this.planMode,
    required this.version,
  });

  factory StatusResponse.fromJson(Map<String, dynamic> json) => StatusResponse(
        sessionId: json['session_id'] as String? ?? '',
        model: json['model'] as String? ?? 'unknown',
        cwd: json['cwd'] as String? ?? '',
        turnCount: json['turn_count'] as int? ?? 0,
        messageCount: json['message_count'] as int? ?? 0,
        costUsd: (json['cost_usd'] as num?)?.toDouble() ?? 0.0,
        planMode: json['plan_mode'] as bool? ?? false,
        version: json['version'] as String? ?? '0.0.0',
      );
}
