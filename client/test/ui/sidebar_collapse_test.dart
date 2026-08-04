import 'package:agent_code_client/agent_code_client.dart';
import 'package:bloc_test/bloc_test.dart';
import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mocktail/mocktail.dart';

import 'package:agent_code_client_app/bloc/session_bloc.dart';
import 'package:agent_code_client_app/bloc/session_event.dart';
import 'package:agent_code_client_app/bloc/session_state.dart';
import 'package:agent_code_client_app/ui/app_theme.dart';
import 'package:agent_code_client_app/ui/sidebar.dart';

class MockSessionBloc extends MockBloc<SessionEvent, SessionState>
    implements SessionBloc {}

class MockWsClient extends Mock implements WsClient {}

SessionData _session(String id) => SessionData(
      id: id,
      instance: AgentInstance(pid: 1, port: 4096, cwd: '/repo', token: 't'),
      wsClient: MockWsClient(),
    );

Widget _wrap(SessionBloc bloc, {VoidCallback? onCollapse}) => MaterialApp(
      theme: buildAppTheme(Brightness.light),
      home: Scaffold(
        body: SizedBox(
          width: 280,
          child: BlocProvider<SessionBloc>.value(
            value: bloc,
            child: Sidebar(onCollapse: onCollapse),
          ),
        ),
      ),
    );

void main() {
  late MockSessionBloc bloc;

  setUp(() {
    bloc = MockSessionBloc();
    whenListen(
      bloc,
      const Stream<SessionState>.empty(),
      initialState: SessionState(sessions: [_session('s1')], activeSessionId: 's1'),
    );
  });

  testWidgets('a collapse control is shown when collapsing is possible',
      (tester) async {
    await tester.pumpWidget(_wrap(bloc, onCollapse: () {}));
    expect(find.byTooltip('Hide sessions'), findsOneWidget);
  });

  testWidgets('tapping the control collapses', (tester) async {
    var collapsed = false;
    await tester.pumpWidget(_wrap(bloc, onCollapse: () => collapsed = true));

    await tester.tap(find.byTooltip('Hide sessions'));
    await tester.pump();

    expect(collapsed, isTrue);
  });

  testWidgets('no control is shown when collapsing is unavailable',
      (tester) async {
    await tester.pumpWidget(_wrap(bloc));
    expect(find.byTooltip('Hide sessions'), findsNothing);
  });

  group('AppTokens', () {
    testWidgets('resolve from the theme', (tester) async {
      late AppTokens tokens;
      await tester.pumpWidget(MaterialApp(
        theme: buildAppTheme(Brightness.light),
        home: Builder(builder: (context) {
          tokens = AppTokens.of(context);
          return const SizedBox();
        }),
      ));

      expect(tokens.sidebarWidth, 280);
      expect(tokens.contentWidth, 820);
      expect(tokens.railWidth, 50);
    });

    testWidgets('fall back on a theme carrying no extension', (tester) async {
      late AppTokens light;
      late AppTokens dark;
      await tester.pumpWidget(MaterialApp(
        theme: ThemeData(brightness: Brightness.light),
        darkTheme: ThemeData(brightness: Brightness.dark),
        home: Builder(builder: (context) {
          light = AppTokens.of(context);
          return Theme(
            data: ThemeData(brightness: Brightness.dark),
            child: Builder(builder: (context) {
              dark = AppTokens.of(context);
              return const SizedBox();
            }),
          );
        }),
      ));

      expect(light.codeBackground, AppTokens.light.codeBackground);
      expect(dark.codeBackground, AppTokens.dark.codeBackground);
    });

    test('lerp interpolates numerics and colors', () {
      final mid = AppTokens.light.lerp(AppTokens.dark, 0.5);
      expect(mid.sidebarWidth, 280);
      expect(mid.codeBackground, isNot(AppTokens.light.codeBackground));
    });

    test('lerp against a foreign extension returns self', () {
      expect(AppTokens.light.lerp(null, 0.5), same(AppTokens.light));
    });

    test('copyWith overrides only what is given', () {
      final wide = AppTokens.light.copyWith(contentWidth: 1000);
      expect(wide.contentWidth, 1000);
      expect(wide.sidebarWidth, AppTokens.light.sidebarWidth);
      expect(wide.accent, AppTokens.light.accent);
    });
  });
}
