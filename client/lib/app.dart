import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import 'bloc/session_bloc.dart';
import 'ui/app_shell.dart';
import 'platform/agent_manager_provider.dart';

class AgentCodeApp extends StatelessWidget {
  const AgentCodeApp({super.key});

  @override
  Widget build(BuildContext context) {
    return BlocProvider(
      create: (_) => SessionBloc(
        agentManager: createAgentManager(),
      ),
      child: MaterialApp(
        title: 'Agent Code',
        debugShowCheckedModeBanner: false,
        theme: _lightTheme(),
        darkTheme: _darkTheme(),
        themeMode: ThemeMode.system,
        home: const AppShell(),
      ),
    );
  }

  ThemeData _lightTheme() => ThemeData(
        brightness: Brightness.light,
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF0071E3),
          brightness: Brightness.light,
        ),
        fontFamily: '.SF Pro Text',
        useMaterial3: true,
      );

  ThemeData _darkTheme() => ThemeData(
        brightness: Brightness.dark,
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF0A84FF),
          brightness: Brightness.dark,
        ),
        fontFamily: '.SF Pro Text',
        useMaterial3: true,
      );
}
