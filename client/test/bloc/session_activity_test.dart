import 'package:flutter_test/flutter_test.dart';

import 'package:agent_code_client_app/bloc/session_activity.dart';

void main() {
  group('SessionActivity', () {
    test('labels', () {
      expect(SessionActivity.idle.label, 'Idle');
      expect(SessionActivity.working.label, 'Working');
      expect(SessionActivity.needsInput.label, 'Needs input');
      expect(SessionActivity.done.label, 'Done');
      expect(SessionActivity.failed.label, 'Failed');
    });

    test('sort order: needs-input first, then working, then idle/finished', () {
      final sorted = [
        SessionActivity.failed,
        SessionActivity.done,
        SessionActivity.idle,
        SessionActivity.working,
        SessionActivity.needsInput,
      ]..sort((a, b) => a.order.compareTo(b.order));

      expect(sorted, [
        SessionActivity.needsInput,
        SessionActivity.working,
        SessionActivity.idle,
        SessionActivity.done,
        SessionActivity.failed,
      ]);
    });

    test('every state has a glyph', () {
      for (final a in SessionActivity.values) {
        expect(a.glyph, isNotEmpty);
      }
    });
  });
}
