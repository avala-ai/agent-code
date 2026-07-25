import 'package:agent_code_client/agent_code_client.dart';
import 'package:bloc_test/bloc_test.dart';
import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mocktail/mocktail.dart';

import 'package:agent_code_client_app/bloc/session_activity.dart';
import 'package:agent_code_client_app/bloc/session_bloc.dart';
import 'package:agent_code_client_app/bloc/session_event.dart';
import 'package:agent_code_client_app/bloc/session_state.dart';
import 'package:agent_code_client_app/ui/sidebar.dart';

class MockSessionBloc extends MockBloc<SessionEvent, SessionState>
    implements SessionBloc {}

class MockWsClient extends Mock implements WsClient {}

SessionData _session(String id, String cwd) => SessionData(
      id: id,
      instance: AgentInstance(pid: 1, port: 4096, cwd: cwd, token: 't'),
      wsClient: MockWsClient(),
    );

Widget _wrap(SessionBloc bloc) => MaterialApp(
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xFF0071E3)),
        useMaterial3: true,
      ),
      home: Scaffold(
        body: SizedBox(
          width: 260,
          child: BlocProvider<SessionBloc>.value(
            value: bloc,
            child: const Sidebar(),
          ),
        ),
      ),
    );

void main() {
  late MockSessionBloc bloc;

  setUp(() => bloc = MockSessionBloc());

  void seed(SessionState state) =>
      whenListen(bloc, const Stream<SessionState>.empty(), initialState: state);

  testWidgets('renders per-session activity labels and dots', (tester) async {
    final state = SessionState(
      sessions: [
        _session('a', '/home/u/alpha'),
        _session('b', '/home/u/beta'),
        _session('c', '/home/u/gamma'),
      ],
      activeSessionId: 'a',
      activity: const {
        'a': SessionActivity.working,
        'b': SessionActivity.needsInput,
        'c': SessionActivity.idle,
      },
    );
    seed(state);

    await tester.pumpWidget(_wrap(bloc));

    // Folder names.
    expect(find.text('alpha'), findsOneWidget);
    expect(find.text('beta'), findsOneWidget);
    expect(find.text('gamma'), findsOneWidget);

    // Per-tile activity labels.
    expect(find.text('Working'), findsOneWidget);
    expect(find.text('Needs input'), findsOneWidget);
    expect(find.text('Idle'), findsOneWidget);
  });

  testWidgets('summary header counts active states, omitting zeros',
      (tester) async {
    final state = SessionState(
      sessions: [
        _session('a', '/home/u/alpha'),
        _session('b', '/home/u/beta'),
        _session('c', '/home/u/gamma'),
      ],
      activeSessionId: 'a',
      activity: const {
        'a': SessionActivity.working,
        'b': SessionActivity.needsInput,
        'c': SessionActivity.idle,
      },
    );
    seed(state);

    await tester.pumpWidget(_wrap(bloc));

    final summary = tester.widget<Text>(find.byKey(const Key('activity-summary')));
    expect(summary.data, contains('1 need input'));
    expect(summary.data, contains('1 working'));
    expect(summary.data, contains('1 idle'));
    // No 'done' or 'failed' since those counts are zero.
    expect(summary.data, isNot(contains('done')));
    expect(summary.data, isNot(contains('failed')));
  });

  testWidgets('summary reads "All idle" when nothing is active',
      (tester) async {
    final state = SessionState(
      sessions: [
        _session('a', '/home/u/alpha'),
        _session('b', '/home/u/beta'),
      ],
      activeSessionId: 'a',
      activity: const {
        'a': SessionActivity.idle,
        'b': SessionActivity.idle,
      },
    );
    seed(state);

    await tester.pumpWidget(_wrap(bloc));

    final summary = tester.widget<Text>(find.byKey(const Key('activity-summary')));
    expect(summary.data, 'All idle');
  });

  testWidgets('no summary header when there are no sessions', (tester) async {
    seed(const SessionState());

    await tester.pumpWidget(_wrap(bloc));

    expect(find.byKey(const Key('activity-summary')), findsNothing);
    expect(find.textContaining('No sessions yet'), findsOneWidget);
  });
}
