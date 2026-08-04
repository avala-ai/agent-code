import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_client_app/ui/app_theme.dart';
import 'package:agent_code_client_app/ui/drafts.dart';

void main() {
  group('Drafts', () {
    test('an unknown key reads as empty', () {
      expect(Drafts().read('nope'), isEmpty);
    });

    test('round-trips a draft per key', () {
      final drafts = Drafts();
      drafts.write('a', 'first');
      drafts.write('b', 'second');
      expect(drafts.read('a'), 'first');
      expect(drafts.read('b'), 'second');
    });

    test('writing empty text drops the entry', () {
      final drafts = Drafts();
      drafts.write('a', 'text');
      drafts.write('a', '');
      expect(drafts.read('a'), isEmpty);
      expect(drafts.length, 0);
    });

    test('clear removes one key only', () {
      final drafts = Drafts();
      drafts.write('a', 'one');
      drafts.write('b', 'two');
      drafts.clear('a');
      expect(drafts.read('a'), isEmpty);
      expect(drafts.read('b'), 'two');
    });

    test('a draft is truncated at the character cap', () {
      final drafts = Drafts();
      drafts.write('a', 'x' * (Drafts.maxChars + 500));
      expect(drafts.read('a').length, Drafts.maxChars);
    });

    test('the store evicts the oldest entry past the cap', () {
      final drafts = Drafts();
      for (var i = 0; i < Drafts.maxDrafts + 5; i++) {
        drafts.write('key$i', 'draft $i');
      }
      expect(drafts.length, Drafts.maxDrafts);
      expect(drafts.read('key0'), isEmpty, reason: 'oldest evicted');
      expect(drafts.read('key${Drafts.maxDrafts + 4}'), isNotEmpty);
    });

    test('rewriting a key refreshes its recency', () {
      final drafts = Drafts();
      for (var i = 0; i < Drafts.maxDrafts; i++) {
        drafts.write('key$i', 'draft $i');
      }
      drafts.write('key0', 'touched');
      drafts.write('overflow', 'new');

      expect(drafts.read('key0'), 'touched', reason: 'refreshed, so not evicted');
      expect(drafts.read('key1'), isEmpty, reason: 'now the oldest');
    });

    test('restore seeds from saved state and honours the cap', () {
      final drafts = Drafts();
      drafts.restore({
        for (var i = 0; i < Drafts.maxDrafts + 3; i++) 'k$i': 'v$i',
      });
      expect(drafts.length, Drafts.maxDrafts);
    });

    test('snapshot is unmodifiable', () {
      final drafts = Drafts();
      drafts.write('a', 'one');
      expect(() => drafts.snapshot()['b'] = 'x', throwsUnsupportedError);
    });

    test('clearAll empties the store', () {
      final drafts = Drafts();
      drafts.write('a', 'one');
      drafts.clearAll();
      expect(drafts.length, 0);
    });

    test('persist is debounced, not called per keystroke', () async {
      var calls = 0;
      final drafts = Drafts(persist: (_) => calls++);

      drafts.write('a', 'h');
      drafts.write('a', 'he');
      drafts.write('a', 'hel');
      expect(calls, 0, reason: 'nothing written before the debounce elapses');

      await Future<void>.delayed(Drafts.persistDebounce * 2);
      expect(calls, 1);
    });

    test('flush writes immediately', () {
      Map<String, String>? saved;
      final drafts = Drafts(persist: (s) => saved = s);
      drafts.write('a', 'urgent');
      expect(saved, isNull);

      drafts.flush();
      expect(saved, isNotNull);
      expect(saved!['a'], 'urgent');
    });

    test('flush with nothing pending is a no-op', () {
      var calls = 0;
      Drafts(persist: (_) => calls++).flush();
      expect(calls, 0);
    });
  });

  group('densityTierFor', () {
    test('a very short pane is a strip', () {
      expect(densityTierFor(800, 80), DensityTier.strip);
    });

    test('a short or narrow pane is a card', () {
      expect(densityTierFor(800, 250), DensityTier.card);
      expect(densityTierFor(200, 800), DensityTier.card);
    });

    test('a medium pane is compact', () {
      expect(densityTierFor(800, 500), DensityTier.compact);
      expect(densityTierFor(400, 800), DensityTier.compact);
    });

    test('a full-size pane is full', () {
      expect(densityTierFor(1200, 900), DensityTier.full);
    });

    test('height wins over width at the boundaries', () {
      expect(densityTierFor(2000, 90), DensityTier.strip);
      expect(densityTierFor(2000, 300), DensityTier.card);
    });
  });
}
