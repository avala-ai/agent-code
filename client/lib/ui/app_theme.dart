import 'package:flutter/material.dart';

/// Design tokens for the client shell.
///
/// Values follow qm's web UI (`plugins/web-ui/src/shell.css`), which is the
/// reference this client's layout is modelled on: a 280px sidebar, a reading
/// column capped at 820px so long replies do not run the full width of a
/// desktop display, an 8/10/16 radius scale, and a 14px/1.5 base type ramp.
@immutable
class AppTokens extends ThemeExtension<AppTokens> {
  /// Width of the session sidebar when open.
  final double sidebarWidth;

  /// Width of the collapsed sidebar. The rail stays in the layout rather than
  /// floating over the transcript, so content can never render underneath the
  /// control that reopens it.
  final double railWidth;

  /// Maximum width of the transcript's reading column.
  final double contentWidth;

  /// Horizontal padding inside the transcript.
  final double chatPadding;

  final double radiusSm;
  final double radiusMd;
  final double radiusLg;

  /// Background for inline code and tool output.
  final Color codeBackground;

  /// Accent used for the brand mark and active affordances. This stays
  /// agent-code's own blue — the layout scale is borrowed, the identity is not.
  final Color accent;

  final String monoFamily;

  const AppTokens({
    this.sidebarWidth = 280,
    this.railWidth = 50,
    this.contentWidth = 820,
    this.chatPadding = 22,
    this.radiusSm = 8,
    this.radiusMd = 10,
    this.radiusLg = 16,
    required this.codeBackground,
    required this.accent,
    this.monoFamily = 'SF Mono',
  });

  static const AppTokens light = AppTokens(
    codeBackground: Color(0xFFF5F5F7),
    accent: Color(0xFF0071E3),
  );

  static const AppTokens dark = AppTokens(
    codeBackground: Color(0xFF2C2C2E),
    accent: Color(0xFF0A84FF),
  );

  /// The tokens for the ambient theme, falling back to the light set so widgets
  /// remain usable in tests that pump a bare [ThemeData].
  static AppTokens of(BuildContext context) {
    final theme = Theme.of(context);
    return theme.extension<AppTokens>() ??
        (theme.brightness == Brightness.dark ? dark : light);
  }

  @override
  AppTokens copyWith({
    double? sidebarWidth,
    double? railWidth,
    double? contentWidth,
    double? chatPadding,
    double? radiusSm,
    double? radiusMd,
    double? radiusLg,
    Color? codeBackground,
    Color? accent,
    String? monoFamily,
  }) =>
      AppTokens(
        sidebarWidth: sidebarWidth ?? this.sidebarWidth,
        railWidth: railWidth ?? this.railWidth,
        contentWidth: contentWidth ?? this.contentWidth,
        chatPadding: chatPadding ?? this.chatPadding,
        radiusSm: radiusSm ?? this.radiusSm,
        radiusMd: radiusMd ?? this.radiusMd,
        radiusLg: radiusLg ?? this.radiusLg,
        codeBackground: codeBackground ?? this.codeBackground,
        accent: accent ?? this.accent,
        monoFamily: monoFamily ?? this.monoFamily,
      );

  @override
  AppTokens lerp(ThemeExtension<AppTokens>? other, double t) {
    if (other is! AppTokens) return this;
    return AppTokens(
      sidebarWidth: _lerpDouble(sidebarWidth, other.sidebarWidth, t),
      railWidth: _lerpDouble(railWidth, other.railWidth, t),
      contentWidth: _lerpDouble(contentWidth, other.contentWidth, t),
      chatPadding: _lerpDouble(chatPadding, other.chatPadding, t),
      radiusSm: _lerpDouble(radiusSm, other.radiusSm, t),
      radiusMd: _lerpDouble(radiusMd, other.radiusMd, t),
      radiusLg: _lerpDouble(radiusLg, other.radiusLg, t),
      codeBackground:
          Color.lerp(codeBackground, other.codeBackground, t) ?? codeBackground,
      accent: Color.lerp(accent, other.accent, t) ?? accent,
      monoFamily: t < 0.5 ? monoFamily : other.monoFamily,
    );
  }

  static double _lerpDouble(double a, double b, double t) => a + (b - a) * t;
}

/// Density tier for a pane, chosen from its rendered size.
///
/// Once panes can be resized or tiled, a transcript can end up 96px tall. The
/// tier lets each surface degrade deliberately at that size instead of
/// overflowing. Thresholds follow qm's `density.ts`.
enum DensityTier { strip, card, compact, full }

const double _stripMaxHeight = 96;
const double _cardMaxHeight = 300;
const double _cardMaxWidth = 250;
const double _compactMaxHeight = 540;
const double _compactMaxWidth = 470;

DensityTier densityTierFor(double width, double height) {
  if (height <= _stripMaxHeight) return DensityTier.strip;
  if (height <= _cardMaxHeight || width <= _cardMaxWidth) return DensityTier.card;
  if (height <= _compactMaxHeight || width <= _compactMaxWidth) {
    return DensityTier.compact;
  }
  return DensityTier.full;
}

ThemeData buildAppTheme(Brightness brightness) {
  final tokens = brightness == Brightness.dark ? AppTokens.dark : AppTokens.light;
  final base = ThemeData(
    brightness: brightness,
    colorScheme: ColorScheme.fromSeed(
      seedColor: tokens.accent,
      brightness: brightness,
    ),
    fontFamily: '.SF Pro Text',
    useMaterial3: true,
  );

  // 14px/1.5 base type, matching the reading rhythm the layout scale assumes.
  return base.copyWith(
    extensions: [tokens],
    textTheme: base.textTheme.copyWith(
      bodyMedium: base.textTheme.bodyMedium?.copyWith(fontSize: 14, height: 1.5),
      bodySmall: base.textTheme.bodySmall?.copyWith(fontSize: 13, height: 1.45),
    ),
  );
}
