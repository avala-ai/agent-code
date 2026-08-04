import 'package:flutter/material.dart';
import 'package:flutter_bloc/flutter_bloc.dart';

import 'bloc/session_bloc.dart';
import 'platform/agent_manager_provider.dart';
import 'ui/app_shell.dart';
import 'ui/app_theme.dart';

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
        theme: buildAppTheme(Brightness.light),
        darkTheme: buildAppTheme(Brightness.dark),
        themeMode: ThemeMode.system,
        home: const AppShell(),
      ),
    );
  }
}
