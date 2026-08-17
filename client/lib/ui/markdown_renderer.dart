import 'package:flutter/material.dart';
import 'package:flutter_markdown/flutter_markdown.dart';

import 'streaming_markdown.dart';

/// Renders markdown, optionally in streaming mode.
///
/// While [streaming] is true the content is split into settled segments plus a
/// live tail (see [splitStreamingMarkdown]). Each settled segment is its own
/// widget with a stable key, so Flutter reuses it untouched as more tokens
/// arrive and only the tail is re-parsed. Handing the whole accumulated string
/// to one [MarkdownBody] on every token is quadratic in the length of the turn.
class MarkdownRenderer extends StatelessWidget {
  final String content;

  /// Whether [content] is still growing. Settled/tail splitting only helps
  /// during a live turn; a finished message renders as a single body.
  final bool streaming;

  const MarkdownRenderer({
    super.key,
    required this.content,
    this.streaming = false,
  });

  @override
  Widget build(BuildContext context) {
    final styleSheet = _styleSheetFor(Theme.of(context));

    if (!streaming || content.length < kSegmentQuantum) {
      return _body(content, styleSheet);
    }

    final split = splitStreamingMarkdown(content);
    if (split.segments.isEmpty) return _body(content, styleSheet);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (var i = 0; i < split.segments.length; i++)
          _SettledSegment(
            // Keyed by index: a segment, once emitted, never changes and never
            // moves, so the index is a stable identity for the life of the turn.
            key: ValueKey('md-seg-$i'),
            markdown: split.segments[i],
            styleSheet: styleSheet,
          ),
        if (split.tail.isNotEmpty) _body(split.tail, styleSheet),
      ],
    );
  }

  Widget _body(String data, MarkdownStyleSheet styleSheet) => MarkdownBody(
        data: data,
        selectable: true,
        styleSheet: styleSheet,
      );

  MarkdownStyleSheet _styleSheetFor(ThemeData theme) {
    final isDark = theme.brightness == Brightness.dark;
    final codeBackground =
        isDark ? const Color(0xFF2C2C2E) : const Color(0xFFF5F5F7);

    return MarkdownStyleSheet(
      p: theme.textTheme.bodyMedium,
      code: TextStyle(
        fontFamily: 'SF Mono',
        fontSize: 12,
        backgroundColor: codeBackground,
      ),
      codeblockDecoration: BoxDecoration(
        color: codeBackground,
        borderRadius: BorderRadius.circular(8),
      ),
      codeblockPadding: const EdgeInsets.all(12),
    );
  }
}

/// A settled markdown segment.
///
/// Const-constructed against an unchanging string, so its element short-circuits
/// rebuilds once the parent's children list stops changing at this index.
class _SettledSegment extends StatelessWidget {
  final String markdown;
  final MarkdownStyleSheet styleSheet;

  const _SettledSegment({
    super.key,
    required this.markdown,
    required this.styleSheet,
  });

  @override
  Widget build(BuildContext context) => MarkdownBody(
        data: markdown,
        selectable: true,
        styleSheet: styleSheet,
      );
}
