import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_client_app/ui/streaming_markdown.dart';

void main() {
  group('stableSplitPoint', () {
    test('nothing has settled in a single paragraph', () {
      expect(stableSplitPoint('just some prose still arriving'), 0);
    });

    test('empty text has no settled prefix', () {
      expect(stableSplitPoint(''), 0);
    });

    test('a blank line between paragraphs settles the first', () {
      const text = 'first para\n\nsecond para';
      final at = stableSplitPoint(text);
      expect(at, greaterThan(0));
      expect(text.substring(0, at), 'first para\n\n');
    });

    test('reports the last of several boundaries', () {
      const text = 'one\n\ntwo\n\nthree';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'one\n\ntwo\n\n');
    });

    test('never splits inside a fenced code block', () {
      const text = 'intro\n\n```dart\nvar a = 1;\n\nvar b = 2;\n';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n');
    });

    test('a closed fence allows a later boundary', () {
      const text = 'intro\n\n```dart\nvar a = 1;\n```\n\nafter';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n```dart\nvar a = 1;\n```\n\n');
    });

    test('a tilde fence is not closed by a backtick fence', () {
      const text = 'intro\n\n~~~\ncode\n```\n\nstill in the tilde fence\n';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n');
    });

    test('an indented fence up to three spaces still counts', () {
      const text = 'intro\n\n   ```\ncode\n\nmore code\n';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n');
    });

    test('never splits between unpaired math delimiters', () {
      const text = 'intro\n\n\$\$\nx = 1\n\ny = 2\n';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n');
    });

    test('paired math delimiters allow a later boundary', () {
      const text = 'intro\n\n\$\$x = 1\$\$\n\nafter';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n\$\$x = 1\$\$\n\n');
    });

    test('never splits an indented block from its continuation', () {
      const text = 'intro\n\n    indented one\n\n    indented two\n';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n');
    });

    test('an indented block ends when unindented text follows', () {
      const text = 'intro\n\n    indented\n\nplain paragraph';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n    indented\n\n');
    });

    test('a tab counts as indentation', () {
      const text = 'intro\n\n\tindented one\n\n\tindented two\n';
      final at = stableSplitPoint(text);
      expect(text.substring(0, at), 'intro\n\n');
    });

    test('a bare trailing newline settles nothing', () {
      // The paragraph may still be growing — one newline does not end it.
      expect(stableSplitPoint('para\n'), 0);
    });

    test('a trailing blank line settles the paragraph before it', () {
      expect(stableSplitPoint('para\n\n'), 6);
    });

    test('a blank first line never yields a zero-length settled prefix', () {
      expect(stableSplitPoint('\ntext'), 0);
    });

    test('the settled prefix only ever grows as text arrives', () {
      const full = 'alpha\n\nbravo\n\ncharlie\n\ndelta';
      var previous = 0;
      for (var i = 1; i <= full.length; i++) {
        final at = stableSplitPoint(full.substring(0, i));
        expect(at, greaterThanOrEqualTo(previous),
            reason: 'settled prefix shrank at length $i');
        previous = at;
      }
    });
  });

  group('splitStreamingMarkdown', () {
    test('short text is all tail', () {
      final split = splitStreamingMarkdown('hello world');
      expect(split.segments, isEmpty);
      expect(split.tail, 'hello world');
    });

    test('segments plus tail always reconstruct the input exactly', () {
      final text = List.generate(80, (i) => 'Paragraph number $i.').join('\n\n');
      final split = splitStreamingMarkdown(text, quantum: 200);
      expect(split.segments.join() + split.tail, text);
    });

    test('every segment reaches the quantum', () {
      final text = List.generate(80, (i) => 'Paragraph number $i.').join('\n\n');
      final split = splitStreamingMarkdown(text, quantum: 200);
      expect(split.segments, isNotEmpty);
      for (final segment in split.segments) {
        expect(segment.length, greaterThanOrEqualTo(200));
      }
    });

    test('no segment ends inside a code fence', () {
      final buffer = StringBuffer();
      for (var i = 0; i < 30; i++) {
        buffer.write('Prose paragraph $i with some length to it.\n\n');
        buffer.write('```dart\nvar x$i = $i;\n\nvar y$i = $i;\n```\n\n');
      }
      final split = splitStreamingMarkdown(buffer.toString(), quantum: 200);
      for (final segment in split.segments) {
        final fences = '```'.allMatches(segment).length;
        expect(fences.isEven, isTrue,
            reason: 'segment ends mid-fence: ${segment.length} chars');
      }
    });

    test('text with no safe boundary stays whole in the tail', () {
      final text = 'x' * 5000;
      final split = splitStreamingMarkdown(text, quantum: 100);
      expect(split.segments, isEmpty);
      expect(split.tail, text);
    });

    test('an unterminated fence keeps everything in the tail', () {
      // The only safe boundary is the one after "intro", which falls short of
      // the quantum, and the open fence swallows every boundary after it. No
      // segment can be emitted, which is the point: nothing gets cut in half.
      final text = 'intro\n\n```\n${'code line\n\n' * 400}';
      final split = splitStreamingMarkdown(text, quantum: 200);
      expect(split.segments, isEmpty);
      expect(split.tail, text);
      expect(stableSplitPoint(text), 7);
    });

    test('growing text never rewrites an already-emitted segment', () {
      final full = List.generate(60, (i) => 'Paragraph $i body text.').join('\n\n');
      List<String> previous = const [];
      for (var i = 100; i <= full.length; i += 37) {
        final split = splitStreamingMarkdown(full.substring(0, i), quantum: 150);
        for (var s = 0; s < previous.length && s < split.segments.length; s++) {
          expect(split.segments[s], previous[s],
              reason: 'segment $s changed at input length $i');
        }
        expect(split.segments.length, greaterThanOrEqualTo(previous.length));
        previous = split.segments;
      }
    });
  });
}
