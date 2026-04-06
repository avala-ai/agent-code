import 'dart:io';

import 'package:toml/toml.dart';

/// Read/write agent-code configuration from ~/.config/agent-code/config.toml.
class ConfigService {
  static const _validPermissionModes = ['default', 'auto', 'plan', 'ask', 'deny'];

  static String? _configPath() {
    if (Platform.isMacOS) {
      final home = Platform.environment['HOME'];
      if (home != null) return '$home/.config/agent-code/config.toml';
    } else if (Platform.isLinux) {
      final xdg = Platform.environment['XDG_CONFIG_HOME'];
      final home = Platform.environment['HOME'];
      final base = xdg ?? (home != null ? '$home/.config' : null);
      if (base != null) return '$base/agent-code/config.toml';
    }
    return null;
  }

  /// Read the current configuration.
  Map<String, dynamic> read() {
    final path = _configPath();
    if (path == null) return {};

    final file = File(path);
    if (!file.existsSync()) return {};

    try {
      final doc = TomlDocument.parse(file.readAsStringSync());
      return doc.toMap();
    } catch (_) {
      return {};
    }
  }

  /// Get the value of a specific config key.
  String? get(String key) {
    final config = read();
    final value = config[key];
    return value?.toString();
  }

  /// Set a config key. Validates permission_mode values.
  void set(String key, String value) {
    if (key == 'permission_mode' && !_validPermissionModes.contains(value)) {
      throw ConfigException(
        'Invalid permission_mode: "$value". '
        'Allowed: $_validPermissionModes',
      );
    }

    final path = _configPath();
    if (path == null) {
      throw ConfigException('Cannot determine config directory');
    }

    final file = File(path);
    final dir = file.parent;
    if (!dir.existsSync()) {
      dir.createSync(recursive: true);
    }

    // Read existing, update, write back.
    Map<String, dynamic> config = {};
    if (file.existsSync()) {
      try {
        config = TomlDocument.parse(file.readAsStringSync()).toMap();
      } catch (_) {
        // Corrupted file, start fresh.
      }
    }

    config[key] = value;

    // Write as simple key = "value" lines (TOML doesn't have a nice writer in Dart).
    final buffer = StringBuffer();
    for (final entry in config.entries) {
      final v = entry.value;
      if (v is String) {
        buffer.writeln('${entry.key} = "${_escapeToml(v)}"');
      } else if (v is bool) {
        buffer.writeln('${entry.key} = $v');
      } else if (v is num) {
        buffer.writeln('${entry.key} = $v');
      }
    }

    file.writeAsStringSync(buffer.toString());
  }

  static String _escapeToml(String s) =>
      s.replaceAll('\\', '\\\\').replaceAll('"', '\\"');
}

class ConfigException implements Exception {
  final String message;
  const ConfigException(this.message);

  @override
  String toString() => 'ConfigException: $message';
}
