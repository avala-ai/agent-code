import 'package:agent_code_client/agent_code_client.dart';

import 'chat_bloc.dart';

/// Keeps one [ChatBloc] per session, alive independently of whether that
/// session's view is on screen.
///
/// A bloc built in a widget's `initState` and closed in its `dispose` dies
/// whenever the view is swapped out — which, for a session list, means
/// switching sessions discards the transcript and unsubscribes from the agent
/// mid-turn. Sessions outlive views, so their state has to as well.
///
/// This is also what lets several sessions render at once: each pane resolves
/// its own bloc from here rather than owning one.
class ChatBlocRegistry {
  final Map<String, ChatBloc> _blocs = {};

  /// The bloc for [sessionId], created against [wsClient] on first request.
  ChatBloc of(String sessionId, WsClient wsClient) =>
      _blocs.putIfAbsent(sessionId, () => ChatBloc(wsClient: wsClient));

  /// The existing bloc for [sessionId], or null if none has been created.
  ChatBloc? peek(String sessionId) => _blocs[sessionId];

  bool has(String sessionId) => _blocs.containsKey(sessionId);

  int get length => _blocs.length;

  /// Closes and forgets the bloc for [sessionId]. Call when the session itself
  /// goes away, never merely because its view unmounted.
  Future<void> remove(String sessionId) async {
    final bloc = _blocs.remove(sessionId);
    await bloc?.close();
  }

  /// Drops every session's state. Retains no keys.
  Future<void> clear() async {
    final blocs = List<ChatBloc>.of(_blocs.values);
    _blocs.clear();
    for (final bloc in blocs) {
      await bloc.close();
    }
  }
}
