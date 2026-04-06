import 'dart:async';
import 'dart:convert';

import 'package:web_socket_channel/web_socket_channel.dart';

import '../models/json_rpc.dart';
import '../models/status_response.dart';

/// WebSocket client that speaks JSON-RPC 2.0 with an agent process.
///
/// Handles bidirectional communication:
/// - Outgoing: requests from Flutter to agent (message, status, cancel)
/// - Incoming: notifications from agent (events/*) and requests (ask_permission)
///
///   Flutter (ws_client)              Agent (WebSocket)
///     │                                │
///     │── Request: message ──────────►│
///     │◄── Notification: text_delta ──│
///     │◄── Notification: tool_start ──│
///     │◄── Request: ask_permission ───│  (agent asks us)
///     │── Response: allow_once ──────►│  (we respond)
///     │◄── Notification: done ────────│
///     │                                │
class WsClient {
  WebSocketChannel? _channel;
  StreamSubscription? _subscription;

  final Map<dynamic, Completer<JsonRpcResponse>> _pending = {};
  int _nextId = 1;

  /// Stream of notifications from the agent (events/text_delta, etc.)
  final _notificationController = StreamController<JsonRpcNotification>.broadcast();
  Stream<JsonRpcNotification> get notifications => _notificationController.stream;

  /// Stream of requests from the agent (ask_permission)
  final _requestController = StreamController<JsonRpcRequest>.broadcast();
  Stream<JsonRpcRequest> get incomingRequests => _requestController.stream;

  /// Whether the WebSocket is currently connected.
  bool get isConnected => _channel != null;

  /// Connect to an agent's WebSocket endpoint.
  Future<void> connect(int port, String token) async {
    await disconnect();

    final uri = Uri.parse('ws://127.0.0.1:$port/ws');
    _channel = WebSocketChannel.connect(
      uri,
      protocols: ['agent-code'],
    );

    // Wait for connection to establish.
    await _channel!.ready;

    // Send auth token as the first message.
    _channel!.sink.add(jsonEncode({'auth': token}));

    _subscription = _channel!.stream.listen(
      _onMessage,
      onError: (error) {
        _handleDisconnect();
      },
      onDone: () {
        _handleDisconnect();
      },
    );
  }

  /// Disconnect from the agent.
  Future<void> disconnect() async {
    await _subscription?.cancel();
    _subscription = null;
    await _channel?.sink.close();
    _channel = null;

    // Fail all pending requests.
    for (final completer in _pending.values) {
      if (!completer.isCompleted) {
        completer.completeError(
          WsClientException('Disconnected while waiting for response'),
        );
      }
    }
    _pending.clear();
  }

  /// Send a message to the agent and wait for the turn to complete.
  /// Events arrive as notifications via the [notifications] stream.
  Future<JsonRpcResponse> sendMessage(String content) {
    return _sendRequest('message', {'content': content});
  }

  /// Get the current session status.
  Future<StatusResponse> getStatus() async {
    final resp = await _sendRequest('status');
    if (resp.error != null) {
      throw WsClientException('Status error: ${resp.error!.message}');
    }
    return StatusResponse.fromJson(resp.result!);
  }

  /// Cancel the currently running turn.
  Future<void> cancel() async {
    await _sendRequest('cancel');
  }

  /// Respond to a permission request from the agent.
  void respondPermission(dynamic requestId, String decision) {
    final response = JsonRpcResponse.success(requestId, {'decision': decision});
    _channel?.sink.add(response.toJson());
  }

  Future<JsonRpcResponse> _sendRequest(
    String method, [
    Map<String, dynamic> params = const {},
  ]) {
    if (_channel == null) {
      throw WsClientException('Not connected');
    }

    final id = _nextId++;
    final request = JsonRpcRequest(id: id, method: method, params: params);
    final completer = Completer<JsonRpcResponse>();
    _pending[id] = completer;

    _channel!.sink.add(request.toJson());
    return completer.future;
  }

  void _onMessage(dynamic data) {
    try {
      final parsed = parseJsonRpc(data as String);

      if (parsed is JsonRpcResponse) {
        // Response to one of our requests.
        final completer = _pending.remove(parsed.id);
        if (completer != null && !completer.isCompleted) {
          completer.complete(parsed);
        }
      } else if (parsed is JsonRpcNotification) {
        // Event from the agent (text_delta, tool_start, done, etc.)
        _notificationController.add(parsed);
      } else if (parsed is JsonRpcRequest) {
        // Request from the agent (ask_permission)
        _requestController.add(parsed);
      }
    } catch (e) {
      // Malformed message, ignore.
    }
  }

  void _handleDisconnect() {
    _channel = null;
    _subscription = null;
    for (final completer in _pending.values) {
      if (!completer.isCompleted) {
        completer.completeError(
          WsClientException('Connection lost'),
        );
      }
    }
    _pending.clear();
  }

  /// Clean up resources.
  Future<void> dispose() async {
    await disconnect();
    await _notificationController.close();
    await _requestController.close();
  }
}

class WsClientException implements Exception {
  final String message;
  const WsClientException(this.message);

  @override
  String toString() => 'WsClientException: $message';
}
