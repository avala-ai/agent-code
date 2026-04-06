import 'package:flutter/material.dart';
import 'package:flutter_markdown/flutter_markdown.dart';

class MarkdownRenderer extends StatelessWidget {
  final String content;

  const MarkdownRenderer({super.key, required this.content});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final isDark = theme.brightness == Brightness.dark;

    return MarkdownBody(
      data: content,
      selectable: true,
      styleSheet: MarkdownStyleSheet(
        p: theme.textTheme.bodyMedium,
        code: TextStyle(
          fontFamily: 'SF Mono',
          fontSize: 12,
          backgroundColor: isDark
              ? const Color(0xFF2C2C2E)
              : const Color(0xFFF5F5F7),
        ),
        codeblockDecoration: BoxDecoration(
          color: isDark ? const Color(0xFF2C2C2E) : const Color(0xFFF5F5F7),
          borderRadius: BorderRadius.circular(8),
        ),
        codeblockPadding: const EdgeInsets.all(12),
      ),
    );
  }
}
