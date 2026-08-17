import 'dart:async';

/// Per-session composer drafts.
///
/// Switching sessions mid-sentence should not lose the sentence. Writes are
/// debounced and the store is capped, because a draft is a convenience and must
/// never grow without bound.
///
/// Ported from qm's `plugins/web-ui/src/drafts.ts`. Persistence to disk is left
/// to a caller that supplies [persist]; without one the store is in-memory and
/// lives as long as the app session.
class Drafts {
  static final Drafts instance = Drafts();

  static const int maxDrafts = 30;
  static const int maxChars = 20000;
  static const Duration persistDebounce = Duration(milliseconds: 400);

  /// Insertion-ordered so the oldest entry is the one evicted at the cap.
  final Map<String, String> _drafts = {};

  /// Called with the whole draft map after writes settle. Optional.
  final FutureOr<void> Function(Map<String, String>)? persist;

  Timer? _timer;

  Drafts({this.persist});

  String read(String key) => _drafts[key] ?? '';

  void write(String key, String text) {
    final trimmed =
        text.length > maxChars ? text.substring(0, maxChars) : text;

    if (trimmed.isEmpty) {
      _drafts.remove(key);
    } else {
      // Re-insert so recency ordering holds for eviction.
      _drafts.remove(key);
      _drafts[key] = trimmed;
      while (_drafts.length > maxDrafts) {
        _drafts.remove(_drafts.keys.first);
      }
    }
    _schedulePersist();
  }

  void clear(String key) {
    if (_drafts.remove(key) != null) _schedulePersist();
  }

  void clearAll() {
    _timer?.cancel();
    _timer = null;
    _drafts.clear();
  }

  /// Seeds the store from previously persisted state.
  void restore(Map<String, String> saved) {
    _drafts
      ..clear()
      ..addAll(saved);
    while (_drafts.length > maxDrafts) {
      _drafts.remove(_drafts.keys.first);
    }
  }

  Map<String, String> snapshot() => Map.unmodifiable(_drafts);

  int get length => _drafts.length;

  void _schedulePersist() {
    if (persist == null) return;
    _timer?.cancel();
    _timer = Timer(persistDebounce, () {
      _timer = null;
      persist!(snapshot());
    });
  }

  /// Flushes any pending debounced write immediately.
  void flush() {
    if (_timer == null) return;
    _timer!.cancel();
    _timer = null;
    persist?.call(snapshot());
  }
}
