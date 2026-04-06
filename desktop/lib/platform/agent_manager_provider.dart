// Conditional import: native platforms get the real AgentManager,
// web gets null (no dart:io available).
export 'agent_manager_native.dart'
    if (dart.library.js_interop) 'agent_manager_web.dart';
