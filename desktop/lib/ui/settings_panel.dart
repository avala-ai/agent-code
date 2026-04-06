import 'package:agent_code_client/agent_code_client.dart';
import 'package:flutter/material.dart';

class SettingsPanel extends StatefulWidget {
  final VoidCallback onClose;

  const SettingsPanel({super.key, required this.onClose});

  @override
  State<SettingsPanel> createState() => _SettingsPanelState();
}

class _SettingsPanelState extends State<SettingsPanel> {
  final _config = ConfigService();
  late final TextEditingController _providerController;
  late final TextEditingController _modelController;
  late final TextEditingController _permissionController;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    final config = _config.read();
    _providerController =
        TextEditingController(text: config['provider']?.toString() ?? '');
    _modelController =
        TextEditingController(text: config['model']?.toString() ?? '');
    _permissionController = TextEditingController(
        text: config['permission_mode']?.toString() ?? '');
  }

  @override
  void dispose() {
    _providerController.dispose();
    _modelController.dispose();
    _permissionController.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    try {
      if (_providerController.text.isNotEmpty) {
        _config.set('provider', _providerController.text);
      }
      if (_modelController.text.isNotEmpty) {
        _config.set('model', _modelController.text);
      }
      if (_permissionController.text.isNotEmpty) {
        _config.set('permission_mode', _permissionController.text);
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Error: $e')),
        );
      }
    }
    if (mounted) setState(() => _saving = false);
  }

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(24),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 500),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Text('Settings',
                    style: Theme.of(context).textTheme.headlineSmall),
                const Spacer(),
                TextButton(
                    onPressed: widget.onClose, child: const Text('Close')),
              ],
            ),
            const SizedBox(height: 20),
            _Field(label: 'Provider', controller: _providerController,
                hint: 'anthropic, openai, etc.'),
            _Field(label: 'Model', controller: _modelController,
                hint: 'claude-sonnet-4, gpt-4.1, etc.'),
            _Field(label: 'Permission Mode', controller: _permissionController,
                hint: 'ask, auto, deny'),
            const SizedBox(height: 16),
            FilledButton(
              onPressed: _saving ? null : _save,
              child: Text(_saving ? 'Saving...' : 'Save Settings'),
            ),
          ],
        ),
      ),
    );
  }
}

class _Field extends StatelessWidget {
  final String label;
  final TextEditingController controller;
  final String hint;

  const _Field({
    required this.label,
    required this.controller,
    required this.hint,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: TextField(
        controller: controller,
        decoration: InputDecoration(
          labelText: label,
          hintText: hint,
          border: const OutlineInputBorder(),
        ),
      ),
    );
  }
}
