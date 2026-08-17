/// Segmentation for markdown that is still being streamed.
///
/// Rendering a growing string by re-parsing the whole thing on every token is
/// quadratic in the length of the turn, and it visibly janks on long replies.
/// The fix is to freeze the part that can no longer change into rendered
/// segments and re-parse only the live tail.
///
/// "Can no longer change" is the whole problem: a split in the wrong place
/// turns one code fence into two broken ones, or splits a `$$…$$` math block,
/// or detaches a paragraph from the indented block it belongs to. A safe
/// boundary is a blank line that is:
///
///   * not inside a fenced code block,
///   * not between an unpaired pair of `$$` math delimiters,
///   * not separating an indented block from a continuation of it.
///
/// Ported from qm's `plugins/web-ui/src/streaming-markdown.ts`.
library;

/// Segments are cut at the first safe boundary at or after this many characters.
/// Small enough that the live tail stays cheap to re-parse; large enough that a
/// long reply does not become thousands of widgets.
const int kSegmentQuantum = 2048;

final RegExp _fence = RegExp(r'^ {0,3}(`{3,}|~{3,})');
final RegExp _indented = RegExp(r'^(?: {4}|\t)');
final RegExp _mathDelim = RegExp(r'\$\$');

/// Walks [text] line by line, tracking fence and math state, and reports every
/// safe boundary to [onBoundary]. Returns as soon as [onBoundary] returns true.
///
/// [onBoundary] receives the offset just past the blank line, which is where a
/// cut may be made.
void _scanBoundaries(String text, bool Function(int offset) onBoundary) {
  final lines = text.split('\n');
  var inFence = false;
  var fenceChar = '';
  var mathDelims = 0;
  var pos = 0;
  var prevIndented = false;

  for (var i = 0; i < lines.length; i++) {
    final line = lines[i];
    final lineStart = pos;
    pos += line.length + 1;

    final fence = _fence.firstMatch(line);
    if (fence != null) {
      final ch = fence.group(1)![0];
      if (!inFence) {
        inFence = true;
        fenceChar = ch;
      } else if (ch == fenceChar) {
        inFence = false;
      }
      continue;
    }
    if (inFence) continue;

    mathDelims += _mathDelim.allMatches(line).length;

    if (line.trim().isNotEmpty) {
      prevIndented = _indented.hasMatch(line);
      continue;
    }

    // A trailing blank line is not a boundary: more text may still arrive.
    if (i == lines.length - 1) continue;
    // Never cut at offset zero, and never inside an open math block.
    if (lineStart == 0 || mathDelims.isOdd) continue;

    // A blank line inside an indented block (code or a lazy continuation) only
    // ends it if what follows is not itself indented.
    if (prevIndented) {
      String? next;
      for (var j = i + 1; j < lines.length; j++) {
        if (lines[j].trim().isNotEmpty) {
          next = lines[j];
          break;
        }
      }
      if (next == null || _indented.hasMatch(next)) continue;
    }

    if (onBoundary(pos < text.length ? pos : text.length)) return;
  }
}

/// The offset of the last point in [text] that is safe to treat as settled.
///
/// Everything before it can be rendered once and never revisited; everything
/// from it onward is still subject to change as more tokens arrive. Returns 0
/// when nothing has settled yet.
int stableSplitPoint(String text) {
  var lastSafe = 0;
  _scanBoundaries(text, (offset) {
    lastSafe = offset;
    return false;
  });
  return lastSafe;
}

/// The result of splitting streamed markdown: [segments] are settled and can be
/// rendered with stable keys, [tail] is the live region still being written.
class StreamingMarkdownSplit {
  final List<String> segments;
  final String tail;

  const StreamingMarkdownSplit(this.segments, this.tail);
}

/// Splits [text] into settled segments of at least [quantum] characters plus the
/// live tail.
///
/// Each segment ends at a safe boundary, so segments can be rendered
/// independently and cached: appending more text never changes an earlier
/// segment, only the tail.
StreamingMarkdownSplit splitStreamingMarkdown(
  String text, {
  int quantum = kSegmentQuantum,
}) {
  final segments = <String>[];
  var segStart = 0;

  while (true) {
    final rest = text.substring(segStart);
    if (rest.length < quantum) break;

    final at = _firstSafeBoundaryAtOrAfter(rest, quantum);
    if (at <= 0) break;

    segments.add(text.substring(segStart, segStart + at));
    segStart += at;
  }

  return StreamingMarkdownSplit(segments, text.substring(segStart));
}

/// The first safe boundary in [text] at or after offset [min], or 0 if there is
/// none.
int _firstSafeBoundaryAtOrAfter(String text, int min) {
  var found = 0;
  _scanBoundaries(text, (offset) {
    if (offset >= min) {
      found = offset;
      return true;
    }
    return false;
  });
  return found;
}
