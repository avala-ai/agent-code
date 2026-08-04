import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'app_theme.dart';
import 'composer_logic.dart';

/// The message composer.
///
/// Two behaviours distinguish it from a plain send box, both taken from qm's
/// web UI:
///
///   * A running turn does not lock the input. The placeholder becomes an
///     invitation to steer and the send button becomes a stop; only an
///     outstanding approval genuinely blocks typing, because only then can the
///     turn not advance.
///   * Typing `/` at a word boundary opens a picker over the agent's
///     user-invocable skills.
class Composer extends StatefulWidget {
  final bool streaming;
  final bool inputBlocked;
  final List<SkillItem> skills;
  final void Function(String text) onSend;
  final VoidCallback? onStop;

  /// Draft text to start from, and a sink for changes so a parent can persist
  /// per-session drafts.
  final String initialDraft;
  final ValueChanged<String>? onDraftChanged;

  const Composer({
    super.key,
    required this.streaming,
    required this.inputBlocked,
    required this.onSend,
    this.skills = const [],
    this.onStop,
    this.initialDraft = '',
    this.onDraftChanged,
  });

  @override
  State<Composer> createState() => _ComposerState();
}

class _ComposerState extends State<Composer> {
  late final TextEditingController _controller;
  final FocusNode _focusNode = FocusNode();

  /// Set when the user dismisses the picker with Escape, so it stays shut until
  /// the slash token changes.
  bool _slashDismissed = false;
  int _slashIndex = 0;

  /// Large pastes held aside as chips rather than dumped into the input.
  final List<_PastedBlock> _pasted = [];

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: widget.initialDraft);
    _controller.addListener(_onDraftChanged);
  }

  @override
  void dispose() {
    _controller.removeListener(_onDraftChanged);
    _controller.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  void _onDraftChanged() {
    widget.onDraftChanged?.call(_controller.text);
    // Re-opening is keyed on the token changing, so a fresh `/` re-offers the
    // picker after a dismissal.
    if (_slashDismissed && slashQuery(_controller.text) == null) {
      _slashDismissed = false;
    }
    setState(() {});
  }

  List<SkillMatch> get _matches {
    if (_slashDismissed) return const [];
    final query = slashQuery(_controller.text);
    if (query == null) return const [];
    return matchSkills(query, widget.skills);
  }

  void _acceptSkill(SkillMatch match) {
    final next = acceptSkill(_controller.text, match.skill);
    _controller.value = TextEditingValue(
      text: next,
      selection: TextSelection.collapsed(offset: next.length),
    );
    setState(() => _slashIndex = 0);
    _focusNode.requestFocus();
  }

  void _send() {
    final typed = _controller.text.trim();
    if (typed.isEmpty && _pasted.isEmpty) return;

    // Chipped pastes are appended as fenced blocks so the agent sees them as
    // the attachments they stand for.
    final buffer = StringBuffer(typed);
    for (final block in _pasted) {
      if (buffer.isNotEmpty) buffer.write('\n\n');
      buffer.write(block.text);
    }

    _controller.clear();
    setState(_pasted.clear);
    widget.onSend(buffer.toString());
  }

  void _handlePaste(String text) {
    if (shouldChipPaste(text)) {
      setState(() => _pasted.add(_PastedBlock(text)));
      return;
    }
    final selection = _controller.selection;
    final cursor = selection.isValid ? selection.baseOffset : null;
    final result = insertIntoDraft(_controller.text, text, cursor);
    _controller.value = TextEditingValue(
      text: result.draft,
      selection: TextSelection.collapsed(offset: result.cursor),
    );
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    final matches = _matches;

    if (matches.isNotEmpty) {
      switch (event.logicalKey) {
        case LogicalKeyboardKey.arrowDown:
          setState(() => _slashIndex = (_slashIndex + 1) % matches.length);
          return KeyEventResult.handled;
        case LogicalKeyboardKey.arrowUp:
          setState(() =>
              _slashIndex = (_slashIndex - 1 + matches.length) % matches.length);
          return KeyEventResult.handled;
        case LogicalKeyboardKey.tab:
        case LogicalKeyboardKey.enter:
        case LogicalKeyboardKey.numpadEnter:
          _acceptSkill(matches[_slashIndex.clamp(0, matches.length - 1)]);
          return KeyEventResult.handled;
        case LogicalKeyboardKey.escape:
          setState(() => _slashDismissed = true);
          return KeyEventResult.handled;
      }
    }

    final isEnter = event.logicalKey == LogicalKeyboardKey.enter ||
        event.logicalKey == LogicalKeyboardKey.numpadEnter;
    if (isEnter &&
        (HardwareKeyboard.instance.isMetaPressed ||
            HardwareKeyboard.instance.isControlPressed)) {
      _send();
      return KeyEventResult.handled;
    }

    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tokens = AppTokens.of(context);
    final matches = _matches;
    final canSend = _controller.text.trim().isNotEmpty || _pasted.isNotEmpty;

    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (matches.isNotEmpty)
          _SlashMenu(
            matches: matches,
            activeIndex: _slashIndex.clamp(0, matches.length - 1),
            onSelect: _acceptSkill,
          ),
        if (_pasted.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: 6),
            child: Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final block in _pasted)
                  _PasteChip(
                    label: pasteChipLabel(block.text.length),
                    onRemove: () => setState(() => _pasted.remove(block)),
                  ),
              ],
            ),
          ),
        Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            Expanded(
              child: Actions(
                actions: {
                  PasteTextIntent: CallbackAction<PasteTextIntent>(
                    onInvoke: (intent) {
                      Clipboard.getData(Clipboard.kTextPlain).then((data) {
                        final text = data?.text;
                        if (text != null && text.isNotEmpty) _handlePaste(text);
                      });
                      return null;
                    },
                  ),
                },
                child: Focus(
                  onKeyEvent: _onKey,
                  child: TextField(
                    controller: _controller,
                    focusNode: _focusNode,
                    maxLines: 8,
                    minLines: 1,
                    enabled: !widget.inputBlocked,
                    decoration: InputDecoration(
                      hintText: composerPlaceholder(
                        inputBlocked: widget.inputBlocked,
                        streaming: widget.streaming,
                      ),
                      border: OutlineInputBorder(
                        borderRadius: BorderRadius.circular(tokens.radiusMd),
                      ),
                      contentPadding: const EdgeInsets.symmetric(
                        horizontal: 14,
                        vertical: 10,
                      ),
                    ),
                  ),
                ),
              ),
            ),
            const SizedBox(width: 8),
            if (widget.streaming && !canSend)
              IconButton.filledTonal(
                onPressed: widget.onStop,
                tooltip: 'Stop the running task',
                icon: const Icon(Icons.stop, size: 18),
              )
            else
              IconButton.filled(
                onPressed: canSend && !widget.inputBlocked ? _send : null,
                tooltip: widget.streaming
                    ? 'Steer the running task'
                    : 'Send (⌘↵)',
                icon: Icon(
                  widget.streaming ? Icons.subdirectory_arrow_right : Icons.arrow_upward,
                  size: 18,
                ),
              ),
          ],
        ),
        if (widget.streaming)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: Text(
              'Working — type to steer',
              style: theme.textTheme.labelSmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
      ],
    );
  }
}

class _PastedBlock {
  final String text;
  const _PastedBlock(this.text);
}

class _PasteChip extends StatelessWidget {
  final String label;
  final VoidCallback onRemove;

  const _PasteChip({required this.label, required this.onRemove});

  @override
  Widget build(BuildContext context) => InputChip(
        label: Text(label),
        avatar: const Icon(Icons.description_outlined, size: 14),
        onDeleted: onRemove,
        deleteIcon: const Icon(Icons.close, size: 14),
        deleteButtonTooltipMessage: 'Remove pasted text',
        visualDensity: VisualDensity.compact,
      );
}

/// The `/` picker, listing matching skills with the typed letters emboldened.
class _SlashMenu extends StatelessWidget {
  final List<SkillMatch> matches;
  final int activeIndex;
  final ValueChanged<SkillMatch> onSelect;

  const _SlashMenu({
    required this.matches,
    required this.activeIndex,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final tokens = AppTokens.of(context);

    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      constraints: const BoxConstraints(maxHeight: 240),
      decoration: BoxDecoration(
        color: theme.colorScheme.surfaceContainer,
        border: Border.all(color: theme.dividerColor),
        borderRadius: BorderRadius.circular(tokens.radiusMd),
      ),
      child: ListView.builder(
        shrinkWrap: true,
        padding: const EdgeInsets.symmetric(vertical: 4),
        itemCount: matches.length,
        itemBuilder: (context, i) {
          final match = matches[i];
          return InkWell(
            onTap: () => onSelect(match),
            child: Container(
              color: i == activeIndex
                  ? theme.colorScheme.surfaceContainerHighest
                  : null,
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
              child: Row(
                children: [
                  _HighlightedName(match: match),
                  if (match.skill.description != null) ...[
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(
                        match.skill.description!,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.bodySmall?.copyWith(
                          color: theme.colorScheme.onSurfaceVariant,
                        ),
                      ),
                    ),
                  ],
                ],
              ),
            ),
          );
        },
      ),
    );
  }
}

/// The skill name with the matched substring emboldened.
class _HighlightedName extends StatelessWidget {
  final SkillMatch match;

  const _HighlightedName({required this.match});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final base = theme.textTheme.bodySmall?.copyWith(
      fontFamily: AppTokens.of(context).monoFamily,
      color: theme.colorScheme.onSurface,
    );
    final name = match.skill.name;

    if (!match.hasSpan) return Text('/$name', style: base);

    return Text.rich(
      TextSpan(
        style: base,
        children: [
          TextSpan(text: '/${name.substring(0, match.start)}'),
          TextSpan(
            text: name.substring(match.start, match.end),
            style: const TextStyle(fontWeight: FontWeight.w700),
          ),
          TextSpan(text: name.substring(match.end)),
        ],
      ),
    );
  }
}
