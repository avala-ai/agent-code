/// Shared client library for Agent Code desktop and mobile apps.
///
/// Provides models, services, and protocol types for communicating
/// with an `agent serve` process over JSON-RPC via WebSocket.
library agent_code_client;

export 'models/agent_instance.dart';
export 'models/chat_message.dart';
export 'models/json_rpc.dart';
export 'models/status_response.dart';
export 'services/agent_manager.dart';
export 'services/ws_client.dart';
export 'services/config_service.dart';
