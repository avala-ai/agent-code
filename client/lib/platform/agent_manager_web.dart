// On web, dart:io is not available. Return null.
// The SessionBloc handles null AgentManager gracefully.

// ignore: avoid_returning_null
dynamic createAgentManager() => null;
