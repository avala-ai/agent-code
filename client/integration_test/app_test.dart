// Integration tests for the Agent Code Flutter client.
//
// These test the real app widget tree — no mocks. They verify that
// the UI renders correctly, buttons are tappable, and state transitions
// work as expected.
//
// Run against Chrome:
//   fvm flutter test integration_test/app_test.dart -d chrome
//
// Run headless (CI):
//   fvm flutter test integration_test/app_test.dart -d web-server

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:agent_code_client_app/main.dart' as app;
import 'package:agent_code_client_app/ui/app_theme.dart';
import 'package:agent_code_client_app/ui/sidebar.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  // ── Layout & Structure ────────────────────────────────────────

  group('App Shell', () {
    testWidgets('renders sidebar and main area', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      // Sidebar header.
      expect(find.text('SESSIONS'), findsOneWidget);

      // Empty state in main area.
      expect(find.text('Agent Code'), findsOneWidget);
      expect(find.text('Create a new session to get started'), findsOneWidget);
    });

    testWidgets('sidebar shows "No sessions yet" when empty', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      expect(find.textContaining('No sessions yet'), findsOneWidget);
      expect(find.textContaining('Click + to start'), findsOneWidget);
    });

    testWidgets('has vertical divider between sidebar and main', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      expect(find.byType(VerticalDivider), findsOneWidget);
    });

    testWidgets('sidebar is laid out at the token width', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final tokens = AppTokens.of(tester.element(find.byType(Sidebar)));
      final width = tester.getSize(find.byType(Sidebar)).width;
      expect(width, tokens.sidebarWidth);
    });

    testWidgets('sidebar collapses to a rail and back', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final tokens = AppTokens.of(tester.element(find.byType(Sidebar)));
      expect(find.text('SESSIONS'), findsOneWidget);

      await tester.tap(find.byTooltip('Hide sessions'));
      await tester.pumpAndSettle();

      // The rail stays in the layout rather than floating over the main area,
      // so the reopen control is always reachable.
      expect(find.text('SESSIONS'), findsNothing);
      final rail = find.byTooltip('Show sessions');
      expect(rail, findsOneWidget);
      expect(
        tester.getSize(find.ancestor(of: rail, matching: find.byType(Column)).first).width,
        lessThanOrEqualTo(tokens.railWidth),
      );

      await tester.tap(rail);
      await tester.pumpAndSettle();
      expect(find.text('SESSIONS'), findsOneWidget);
    });
  });

  // ── New Session Button ────────────────────────────────────────

  group('+ New Button', () {
    testWidgets('is visible in sidebar', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final newButton = find.text('+ New');
      expect(newButton, findsOneWidget);
    });

    testWidgets('is a TextButton widget', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final button = find.widgetWithText(TextButton, '+ New');
      expect(button, findsOneWidget);
    });

    // Note: button tap tests (error on web, error display) are NOT run here.
    // Tapping + New triggers _pickFolderAndCreate which is async and uses
    // Directory.current (dart:io). On web, this fires an async error that
    // outlives the test teardown, causing "inTest is not true" assertions
    // that poison all subsequent tests. The tap-to-error flow is covered
    // by Playwright E2E tests (PR #84) which handle async naturally.
  });

  // ── Theme ─────────────────────────────────────────────────────

  group('Theme', () {
    testWidgets('app uses Material 3', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final materialApp = tester.widget<MaterialApp>(find.byType(MaterialApp));
      expect(materialApp.theme?.useMaterial3, isTrue);
      expect(materialApp.darkTheme?.useMaterial3, isTrue);
    });

    testWidgets('debug banner is hidden', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final materialApp = tester.widget<MaterialApp>(find.byType(MaterialApp));
      expect(materialApp.debugShowCheckedModeBanner, isFalse);
    });

    testWidgets('supports dark and light mode', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      final materialApp = tester.widget<MaterialApp>(find.byType(MaterialApp));
      expect(materialApp.themeMode, ThemeMode.system);
      expect(materialApp.theme, isNotNull);
      expect(materialApp.darkTheme, isNotNull);
    });
  });

  // ── Widget Tree Integrity ─────────────────────────────────────

  group('Widget Tree', () {
    testWidgets('has BlocProvider for SessionBloc', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      // BlocBuilder inside Sidebar and AppShell read SessionBloc.
      // If BlocProvider is missing, the app would crash.
      // The fact that the app renders proves BlocProvider exists.
      expect(find.text('SESSIONS'), findsOneWidget);
    });

    testWidgets('Row contains sidebar and expanded main area', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      // The AppShell has a Row with the sidebar + VerticalDivider + Expanded.
      expect(find.byType(Row), findsWidgets);
      expect(find.byType(Expanded), findsWidgets);
    });

    testWidgets('no overflow errors on standard screen size', (tester) async {
      // Default test window is 800x600. Should render without overflow.
      app.main();
      await tester.pumpAndSettle();

      // If there's an overflow, Flutter will report an exception.
      // pumpAndSettle would throw. Reaching here means no overflow.
      expect(find.byType(MaterialApp), findsOneWidget);
    });
  });

  // ── Accessibility ─────────────────────────────────────────────

  group('Accessibility', () {
    testWidgets('+ New button has semantic label', (tester) async {
      app.main();
      await tester.pumpAndSettle();

      // The button text "+ New" serves as the semantic label.
      final button = find.text('+ New');
      expect(button, findsOneWidget);

      // Verify it's inside a tappable widget.
      expect(find.widgetWithText(TextButton, '+ New'), findsOneWidget);
    });
  });
}
