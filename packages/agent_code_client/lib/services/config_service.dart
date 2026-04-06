import 'dart:io';

/// Read/write agent-code configuration from ~/.config/agent-code/config.toml.
///
/// Uses a simple key=value parser since the config is flat (no nested tables).
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

  /// Read the current configuration as a flat map.
  Map<String, String> read() {
    final path = _configPath();
    if (path == null) return {};

    final file = File(path);
    if (!file.existsSync()) return {};

    try {
      return _parseToml(file.readAsStringSync());
    } catch (_) {
      return {};
    }
  }

  /// Get the value of a specific config key.
  String? get(String key) => read()[key];

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
      throw const ConfigException('Cannot determine config directory');
    }

    final file = File(path);
    final dir = file.parent;
    if (!dir.existsSync()) {
      dir.createSync(recursive: true);
    }

    final config = read();
    config[key] = value;

    final buffer = StringBuffer();
    for (final entry in config.entries) {
      buffer.writeln('${entry.key} = "${_escapeToml(entry.value)}"');
    }

    file.writeAsStringSync(buffer.toString());
  }

  /// Simple flat TOML parser. Handles `key = "value"` and `key = value`.
  static Map<String, String> _parseToml(String content) {
    final result = <String, String>{};
    for (final line in content.split('\n')) {
      final trimmed = line.trim();
      if (trimmed.isEmpty || trimmed.startsWith('#') || trimmed.startsWith('[')) {
        continue;
      }
      final eqIndex = trimmed.indexOf('=');
      if (eqIndex < 0) continue;

      final key = trimmed.substring(0, eqIndex).trim();
      var value = trimmed.substring(eqIndex + 1).trim();

      // Strip surrounding quotes.
      if (value.startsWith('"') && value.endsWith('"') && value.length >= 2) {
        value = value.substring(1, value.length - 1);
        value = value.replaceAll('\\"', '"').replaceAll('\\\\', '\\');
      }

      result[key] = value;
    }
    return result;
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
