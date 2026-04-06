/// Shared client library for Agent Code desktop and mobile apps.
///
/// Provides models, services, and protocol types for communicating
/// with an `agent serve` process over JSON-RPC via WebSocket.
///
/// Note: [AgentManager] and [ConfigService] use dart:io and are not
/// available on web. Import them directly when needed:
///   import 'package:agent_code_client/services/agent_manager.dart';
///   import 'package:agent_code_client/services/config_service.dart';
library agent_code_client;

// Models (platform-independent)
export 'models/agent_instance.dart';
export 'models/chat_message.dart';
export 'models/json_rpc.dart';
export 'models/status_response.dart';

// Services (platform-independent)
export 'services/ws_client.dart';
export 'services/update_checker.dart';

// NOTE: agent_manager.dart and config_service.dart use dart:io
// and must be imported directly (not from this barrel) on native platforms.
