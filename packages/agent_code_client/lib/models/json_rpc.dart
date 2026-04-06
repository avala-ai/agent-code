import 'dart:convert';

/// A JSON-RPC 2.0 request (has an id, expects a response).
class JsonRpcRequest {
  final dynamic id;
  final String method;
  final Map<String, dynamic> params;

  const JsonRpcRequest({
    required this.id,
    required this.method,
    this.params = const {},
  });

  String toJson() => jsonEncode({
        'jsonrpc': '2.0',
        'id': id,
        'method': method,
        if (params.isNotEmpty) 'params': params,
      });
}

/// A JSON-RPC 2.0 response (matches a request by id).
class JsonRpcResponse {
  final dynamic id;
  final Map<String, dynamic>? result;
  final JsonRpcError? error;

  const JsonRpcResponse({required this.id, this.result, this.error});

  factory JsonRpcResponse.success(dynamic id, Map<String, dynamic> result) =>
      JsonRpcResponse(id: id, result: result);

  factory JsonRpcResponse.error(dynamic id, int code, String message) =>
      JsonRpcResponse(id: id, error: JsonRpcError(code: code, message: message));

  String toJson() => jsonEncode({
        'jsonrpc': '2.0',
        'id': id,
        if (result != null) 'result': result,
        if (error != null)
          'error': {'code': error!.code, 'message': error!.message},
      });
}

class JsonRpcError {
  final int code;
  final String message;

  const JsonRpcError({required this.code, required this.message});
}

/// A JSON-RPC 2.0 notification (no id, no response expected).
class JsonRpcNotification {
  final String method;
  final Map<String, dynamic> params;

  const JsonRpcNotification({required this.method, this.params = const {}});

  String toJson() => jsonEncode({
        'jsonrpc': '2.0',
        'method': method,
        if (params.isNotEmpty) 'params': params,
      });
}

/// Parse a raw JSON-RPC message into the appropriate type.
///
/// Returns [JsonRpcRequest] if it has both `id` and `method`,
/// [JsonRpcResponse] if it has `id` but no `method`,
/// [JsonRpcNotification] if it has `method` but no `id`.
Object parseJsonRpc(String raw) {
  final json = jsonDecode(raw) as Map<String, dynamic>;
  final hasId = json.containsKey('id') && json['id'] != null;
  final hasMethod = json.containsKey('method');

  if (hasId && hasMethod) {
    // Request from agent (e.g., ask_permission)
    return JsonRpcRequest(
      id: json['id'],
      method: json['method'] as String,
      params: (json['params'] as Map<String, dynamic>?) ?? {},
    );
  } else if (hasId) {
    // Response to our request
    JsonRpcError? error;
    if (json.containsKey('error') && json['error'] != null) {
      final e = json['error'] as Map<String, dynamic>;
      error = JsonRpcError(
        code: e['code'] as int,
        message: e['message'] as String,
      );
    }
    return JsonRpcResponse(
      id: json['id'],
      result: json['result'] as Map<String, dynamic>?,
      error: error,
    );
  } else if (hasMethod) {
    // Notification (events/text_delta, events/tool_start, etc.)
    return JsonRpcNotification(
      method: json['method'] as String,
      params: (json['params'] as Map<String, dynamic>?) ?? {},
    );
  }

  throw FormatException('Invalid JSON-RPC message: $raw');
}
