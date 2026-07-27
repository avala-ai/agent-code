//! Runtime facade for terminal themes.

use std::sync::RwLock;

use crossterm::style::{Color, StyledContent};

#[path = "theme.rs"]
mod legacy;

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub muted: Color,
    pub inactive: Color,
    pub tool: Color,
    pub plan: Color,
    pub text: Color,
    /// The theme's own background.
    pub bg: Color,
    /// Inline-code and code-block foreground / background.
    pub code_fg: Color,
    pub code_bg: Color,
    pub diff_add: Color,
    pub diff_remove: Color,
    pub agent_colors: [Color; 8],

    // Shimmer variants — see [`legacy::Theme`] for slot semantics.
    pub success_shimmer: Color,
    pub error_shimmer: Color,
    pub warning_shimmer: Color,
    pub accent_shimmer: Color,
    pub muted_shimmer: Color,

    // Diff intensity variants.
    pub diff_added_dimmed: Color,
    pub diff_removed_dimmed: Color,
    pub diff_added_word: Color,
    pub diff_removed_word: Color,

    // Stable subagent identification colors.
    pub subagent_red: Color,
    pub subagent_blue: Color,
    pub subagent_green: Color,
    pub subagent_yellow: Color,
    pub subagent_purple: Color,
    pub subagent_orange: Color,
    pub subagent_pink: Color,
    pub subagent_cyan: Color,

    // Rate-limit / budget bar.
    pub rate_limit_fill: Color,
    pub rate_limit_empty: Color,

    // Selection / hover / interaction backgrounds.
    pub selection_bg: Color,
    pub message_action_bg: Color,
    pub user_message_bg: Color,
    pub bash_message_bg: Color,
    pub memory_message_bg: Color,

    // Rainbow keyword highlighting.
    pub rainbow_red: Color,
    pub rainbow_orange: Color,
    pub rainbow_yellow: Color,
    pub rainbow_green: Color,
    pub rainbow_blue: Color,
    pub rainbow_indigo: Color,
    pub rainbow_violet: Color,

    // Mode tags for the per-mode REPL prompt.
    pub plan_mode: Color,
    pub brief_mode: Color,
    pub fast_mode: Color,
    pub fast_mode_shimmer: Color,

    pub is_dark: bool,
}

impl Theme {
    /// Resolve a theme name to a runtime [`Theme`]. `auto` is resolved
    /// here (rather than delegated) so it uses the richer terminal
    /// background detection from [`detect_system_theme`]; every other
    /// name — including the `dark`/`light` aliases and user palette ids
    /// — is resolved by the data-driven [`legacy::Theme::from_name`].
    pub fn from_name(name: &str) -> Self {
        if name == "auto" {
            let id = if detect_system_theme() == "light" {
                "solarized-light"
            } else {
                "one-dark"
            };
            return legacy::Theme::from_name(id).into();
        }
        legacy::Theme::from_name(name).into()
    }

    pub fn all_names() -> Vec<String> {
        legacy::Theme::all_names()
    }

    /// `(id, label)` pairs in display order — for the theme picker.
    pub fn all_options() -> Vec<(String, String)> {
        legacy::Theme::all_options()
    }

    pub fn agent_color(&self, index: usize) -> Color {
        self.agent_colors[index % self.agent_colors.len()]
    }
}

impl From<legacy::Theme> for Theme {
    fn from(theme: legacy::Theme) -> Self {
        Self {
            accent: theme.accent,
            error: theme.error,
            warning: theme.warning,
            success: theme.success,
            muted: theme.muted,
            inactive: theme.inactive,
            tool: theme.tool,
            plan: theme.plan,
            text: theme.text,
            bg: theme.bg,
            code_fg: theme.code_fg,
            code_bg: theme.code_bg,
            diff_add: theme.diff_add,
            diff_remove: theme.diff_remove,
            agent_colors: theme.agent_colors,
            success_shimmer: theme.success_shimmer,
            error_shimmer: theme.error_shimmer,
            warning_shimmer: theme.warning_shimmer,
            accent_shimmer: theme.accent_shimmer,
            muted_shimmer: theme.muted_shimmer,
            diff_added_dimmed: theme.diff_added_dimmed,
            diff_removed_dimmed: theme.diff_removed_dimmed,
            diff_added_word: theme.diff_added_word,
            diff_removed_word: theme.diff_removed_word,
            subagent_red: theme.subagent_red,
            subagent_blue: theme.subagent_blue,
            subagent_green: theme.subagent_green,
            subagent_yellow: theme.subagent_yellow,
            subagent_purple: theme.subagent_purple,
            subagent_orange: theme.subagent_orange,
            subagent_pink: theme.subagent_pink,
            subagent_cyan: theme.subagent_cyan,
            rate_limit_fill: theme.rate_limit_fill,
            rate_limit_empty: theme.rate_limit_empty,
            selection_bg: theme.selection_bg,
            message_action_bg: theme.message_action_bg,
            user_message_bg: theme.user_message_bg,
            bash_message_bg: theme.bash_message_bg,
            memory_message_bg: theme.memory_message_bg,
            rainbow_red: theme.rainbow_red,
            rainbow_orange: theme.rainbow_orange,
            rainbow_yellow: theme.rainbow_yellow,
            rainbow_green: theme.rainbow_green,
            rainbow_blue: theme.rainbow_blue,
            rainbow_indigo: theme.rainbow_indigo,
            rainbow_violet: theme.rainbow_violet,
            plan_mode: theme.plan_mode,
            brief_mode: theme.brief_mode,
            fast_mode: theme.fast_mode,
            fast_mode_shimmer: theme.fast_mode_shimmer,
            is_dark: theme.is_dark,
        }
    }
}

/// Shared contrast utilities. Live on the palette side because that is
/// where derived slots are computed; re-exported so the ratatui-side
/// chrome measures legibility the same way.
pub(crate) use legacy::{MIN_TEXT_CONTRAST, contrast_ratio, relative_luminance};

pub fn styled(text: &str, color: Color) -> StyledContent<String> {
    legacy::styled(text, color)
}

/// Resolve a [`SubagentColor`] slot against the supplied theme.
///
/// Returns the theme's `subagent_*` slot for the variant. Lives on
/// the runtime facade rather than the legacy struct because
/// `SubagentColor` is defined in `agent-code-lib` (the engine has to
/// hand out colours when it spawns subagents — the CLI is downstream)
/// and the mapping needs both that type *and* the runtime `Theme`.
pub fn subagent_theme_color(
    color: agent_code_lib::services::subagent_colors::SubagentColor,
    theme: &Theme,
) -> Color {
    use agent_code_lib::services::subagent_colors::SubagentColor;
    match color {
        SubagentColor::Red => theme.subagent_red,
        SubagentColor::Blue => theme.subagent_blue,
        SubagentColor::Green => theme.subagent_green,
        SubagentColor::Yellow => theme.subagent_yellow,
        SubagentColor::Purple => theme.subagent_purple,
        SubagentColor::Orange => theme.subagent_orange,
        SubagentColor::Pink => theme.subagent_pink,
        SubagentColor::Cyan => theme.subagent_cyan,
    }
}

pub fn styled_bold(text: &str, color: Color) -> StyledContent<String> {
    legacy::styled_bold(text, color)
}

pub fn label(text: &str, bg: Color, fg: Color) -> StyledContent<String> {
    legacy::label(text, bg, fg)
}

/// Detect whether the terminal has a light background.
pub fn detect_system_theme() -> &'static str {
    super::terminal_query::system_theme().as_str()
}

/// Resolve a config theme name, handling "auto".
pub fn resolve_theme(configured: &str) -> String {
    if configured == "auto" {
        if detect_system_theme() == "light" {
            "solarized-light".to_string()
        } else {
            "one-dark".to_string()
        }
    } else {
        configured.to_string()
    }
}

/// Options recorded alongside the active theme so we can replay the
/// "auto + inherit_fg" override path when callers ask for `current()`.
/// Stored as a separate slot rather than baked into the [`Theme`]
/// struct because the override is dynamic — the OSC 10 cache may
/// populate after `init` has already run.
#[derive(Debug, Clone, Copy, Default)]
struct ActiveOptions {
    /// True when the user's configured theme name was `auto` — the
    /// only situation in which `inherit_fg` should adjust the `text`
    /// slot. Other themes are explicit choices and we honour them
    /// verbatim.
    auto: bool,
    /// True when `[ui].inherit_fg = true` in config.
    inherit_fg: bool,
}

static ACTIVE_THEME: RwLock<Option<Theme>> = RwLock::new(None);
static ACTIVE_OPTIONS: RwLock<ActiveOptions> = RwLock::new(ActiveOptions {
    auto: false,
    inherit_fg: false,
});

/// Serializes tests that mutate the process-global active theme.
///
/// [`init`] writes a `static RwLock`, so two tests that set different
/// themes and then assert on [`current`] race under the default parallel
/// test runner. Every test that calls `init*` must hold this guard.
/// Poisoning is ignored: a panicking test should fail on its own
/// assertion, not cascade into unrelated ones.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static THEME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    THEME_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Initialize (or re-set) the global theme. Convenience wrapper that
/// disables the inherit-fg override; callers that want the override
/// should reach for [`init_with_options`] and pass the configured
/// (pre-resolution) theme name plus the `inherit_fg` flag.
pub fn init(theme_name: &str) {
    init_with_options(theme_name, theme_name, false);
}

/// Initialize the global theme with the inherit-fg override.
///
/// `configured_name` is the *user-typed* theme value (`"auto"`,
/// `"midnight"`, …); `theme_name` is what [`resolve_theme`] returned
/// for it. We need both because `inherit_fg` only fires when the
/// user opted into Auto — explicit themes are taken at face value.
pub fn init_with_options(theme_name: &str, configured_name: &str, inherit_fg: bool) {
    let theme = Theme::from_name(theme_name);
    if let Ok(mut guard) = ACTIVE_THEME.write() {
        *guard = Some(theme);
    }
    if let Ok(mut guard) = ACTIVE_OPTIONS.write() {
        *guard = ActiveOptions {
            auto: configured_name == "auto",
            inherit_fg,
        };
    }
}

/// Get a snapshot of the active theme. Each color slot is adapted to
/// the current [`color_emit::EmitMode`] so consumers transparently get
/// the right palette without threading the mode through every render
/// callsite. The stored theme keeps its original RGB values; only this
/// snapshot is downgraded.
///
/// When the user picked Auto and enabled `inherit_fg`, the `text`
/// slot is replaced with the foreground RGB the terminal reported via
/// OSC 10. If detection failed (cache empty) the theme default is
/// preserved instead.
pub fn current() -> Theme {
    let raw = ACTIVE_THEME
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| Theme::from_name("dark"));
    let options = ACTIVE_OPTIONS.read().map(|g| *g).unwrap_or_default();
    adapt_for_emit(apply_inherit_fg(raw, options))
}

fn apply_inherit_fg(mut theme: Theme, options: ActiveOptions) -> Theme {
    apply_inherit_fg_with(
        &mut theme,
        options,
        super::terminal_query::detect_terminal_foreground(),
    );
    theme
}

/// Pure variant of [`apply_inherit_fg`] used by both the live path and
/// the unit tests. Splitting the override away from the cache lookup
/// is the only way to exercise the "detection succeeded / detection
/// failed / inherit-fg disabled" matrix without racing on a global
/// `OnceLock`.
fn apply_inherit_fg_with(
    theme: &mut Theme,
    options: ActiveOptions,
    detected: Option<(u8, u8, u8)>,
) {
    if options.auto
        && options.inherit_fg
        && let Some((r, g, b)) = detected
    {
        theme.text = Color::Rgb { r, g, b };
    }
}

fn adapt_for_emit(theme: Theme) -> Theme {
    adapt_for_emit_with(theme, super::color_emit::current())
}

/// Pure variant of [`adapt_for_emit`]. Every palette slot the UI reads
/// goes through here, so a mode change (256-colour downgrade, or the
/// monochrome mode `NO_COLOR` selects) applies uniformly instead of
/// per-callsite.
fn adapt_for_emit_with(theme: Theme, mode: super::color_emit::EmitMode) -> Theme {
    if mode == super::color_emit::EmitMode::Truecolor {
        return theme;
    }
    let adapt = |c: Color| super::color_emit::adapt(mode, c);
    Theme {
        accent: adapt(theme.accent),
        error: adapt(theme.error),
        warning: adapt(theme.warning),
        success: adapt(theme.success),
        muted: adapt(theme.muted),
        inactive: adapt(theme.inactive),
        tool: adapt(theme.tool),
        plan: adapt(theme.plan),
        text: adapt(theme.text),
        bg: adapt(theme.bg),
        code_fg: adapt(theme.code_fg),
        code_bg: adapt(theme.code_bg),
        diff_add: adapt(theme.diff_add),
        diff_remove: adapt(theme.diff_remove),
        agent_colors: theme.agent_colors.map(adapt),
        success_shimmer: adapt(theme.success_shimmer),
        error_shimmer: adapt(theme.error_shimmer),
        warning_shimmer: adapt(theme.warning_shimmer),
        accent_shimmer: adapt(theme.accent_shimmer),
        muted_shimmer: adapt(theme.muted_shimmer),
        diff_added_dimmed: adapt(theme.diff_added_dimmed),
        diff_removed_dimmed: adapt(theme.diff_removed_dimmed),
        diff_added_word: adapt(theme.diff_added_word),
        diff_removed_word: adapt(theme.diff_removed_word),
        subagent_red: adapt(theme.subagent_red),
        subagent_blue: adapt(theme.subagent_blue),
        subagent_green: adapt(theme.subagent_green),
        subagent_yellow: adapt(theme.subagent_yellow),
        subagent_purple: adapt(theme.subagent_purple),
        subagent_orange: adapt(theme.subagent_orange),
        subagent_pink: adapt(theme.subagent_pink),
        subagent_cyan: adapt(theme.subagent_cyan),
        rate_limit_fill: adapt(theme.rate_limit_fill),
        rate_limit_empty: adapt(theme.rate_limit_empty),
        selection_bg: adapt(theme.selection_bg),
        message_action_bg: adapt(theme.message_action_bg),
        user_message_bg: adapt(theme.user_message_bg),
        bash_message_bg: adapt(theme.bash_message_bg),
        memory_message_bg: adapt(theme.memory_message_bg),
        rainbow_red: adapt(theme.rainbow_red),
        rainbow_orange: adapt(theme.rainbow_orange),
        rainbow_yellow: adapt(theme.rainbow_yellow),
        rainbow_green: adapt(theme.rainbow_green),
        rainbow_blue: adapt(theme.rainbow_blue),
        rainbow_indigo: adapt(theme.rainbow_indigo),
        rainbow_violet: adapt(theme.rainbow_violet),
        plan_mode: adapt(theme.plan_mode),
        brief_mode: adapt(theme.brief_mode),
        fast_mode: adapt(theme.fast_mode),
        fast_mode_shimmer: adapt(theme.fast_mode_shimmer),
        is_dark: theme.is_dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_mode_strips_every_palette_slot() {
        use super::super::color_emit::EmitMode;
        let theme = adapt_for_emit_with(Theme::from_name("one-dark"), EmitMode::Mono);
        let slots: Vec<(&str, Color)> = vec![
            ("accent", theme.accent),
            ("error", theme.error),
            ("warning", theme.warning),
            ("success", theme.success),
            ("muted", theme.muted),
            ("inactive", theme.inactive),
            ("tool", theme.tool),
            ("plan", theme.plan),
            ("text", theme.text),
            ("bg", theme.bg),
            ("code_fg", theme.code_fg),
            ("code_bg", theme.code_bg),
            ("diff_add", theme.diff_add),
            ("diff_remove", theme.diff_remove),
            ("selection_bg", theme.selection_bg),
            ("user_message_bg", theme.user_message_bg),
            ("rate_limit_fill", theme.rate_limit_fill),
            ("rainbow_red", theme.rainbow_red),
            ("plan_mode", theme.plan_mode),
            ("fast_mode", theme.fast_mode),
        ];
        for (name, color) in slots {
            assert_eq!(color, Color::Reset, "slot {name} still carries colour");
        }
        for (i, c) in theme.agent_colors.iter().enumerate() {
            assert_eq!(*c, Color::Reset, "agent_colors[{i}] still carries colour");
        }
    }

    #[test]
    fn non_mono_downgrade_keeps_slots_distinguishable() {
        use super::super::color_emit::EmitMode;
        let theme = adapt_for_emit_with(Theme::from_name("one-dark"), EmitMode::Ansi256);
        assert_ne!(theme.accent, Color::Reset);
        assert_ne!(theme.error, theme.success);
    }

    #[test]
    fn resolve_non_auto_preserves_configured_name() {
        assert_eq!(resolve_theme("dracula"), "dracula");
        assert_eq!(resolve_theme("solarized-light"), "solarized-light");
    }

    #[test]
    fn every_advertised_theme_resolves_through_facade() {
        for name in Theme::all_names() {
            let theme = Theme::from_name(&name);
            assert_eq!(theme.agent_colors.len(), 8, "theme {name}");
        }
    }

    /// Helper: build the `ActiveOptions` matrix value once.
    fn opts(auto: bool, inherit_fg: bool) -> ActiveOptions {
        ActiveOptions { auto, inherit_fg }
    }

    #[test]
    fn inherit_fg_overrides_text_when_detection_succeeds() {
        // Auto + inherit_fg + cached foreground → text slot becomes
        // the detected RGB, every other slot is left alone.
        let mut theme = Theme::from_name("dark");
        let original_accent = theme.accent;
        apply_inherit_fg_with(&mut theme, opts(true, true), Some((0x12, 0x34, 0x56)));
        assert_eq!(
            theme.text,
            Color::Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        );
        // Sibling slots untouched — the override is fg-only.
        assert_eq!(theme.accent, original_accent);
    }

    #[test]
    fn inherit_fg_falls_back_to_theme_text_when_detection_fails() {
        // Auto + inherit_fg but no cached foreground → keep the
        // theme's own text colour. This is the "we didn't get an OSC
        // 10 reply" branch.
        let mut theme = Theme::from_name("dark");
        let default_text = theme.text;
        apply_inherit_fg_with(&mut theme, opts(true, true), None);
        assert_eq!(theme.text, default_text);
    }

    #[test]
    fn inherit_fg_disabled_keeps_theme_default_even_with_cache_hit() {
        // Auto theme but inherit_fg = false → ignore the cache. This
        // is the default behaviour and protects users who explicitly
        // chose Auto without opting into the override.
        let mut theme = Theme::from_name("dark");
        let default_text = theme.text;
        apply_inherit_fg_with(&mut theme, opts(true, false), Some((0xAA, 0xBB, 0xCC)));
        assert_eq!(theme.text, default_text);
    }

    #[test]
    fn inherit_fg_only_fires_when_configured_theme_was_auto() {
        // Explicit theme + inherit_fg = true → still ignore the
        // cache. The override is Auto-only because users who picked
        // Midnight (etc.) intentionally signed up for that palette.
        let mut theme = Theme::from_name("dark");
        let default_text = theme.text;
        apply_inherit_fg_with(&mut theme, opts(false, true), Some((0xAA, 0xBB, 0xCC)));
        assert_eq!(theme.text, default_text);
    }

    /// Each `SubagentColor` variant should resolve to the matching
    /// `subagent_*` slot for any theme. Run across every advertised
    /// theme so a future palette regression surfaces immediately.
    #[test]
    fn subagent_theme_color_maps_to_each_named_slot() {
        use agent_code_lib::services::subagent_colors::SubagentColor;
        for name in Theme::all_names() {
            let theme = Theme::from_name(&name);
            assert_eq!(
                subagent_theme_color(SubagentColor::Red, &theme),
                theme.subagent_red,
                "red mismatch on {name}"
            );
            assert_eq!(
                subagent_theme_color(SubagentColor::Blue, &theme),
                theme.subagent_blue,
                "blue mismatch on {name}"
            );
            assert_eq!(
                subagent_theme_color(SubagentColor::Green, &theme),
                theme.subagent_green,
                "green mismatch on {name}"
            );
            assert_eq!(
                subagent_theme_color(SubagentColor::Yellow, &theme),
                theme.subagent_yellow,
                "yellow mismatch on {name}"
            );
            assert_eq!(
                subagent_theme_color(SubagentColor::Purple, &theme),
                theme.subagent_purple,
                "purple mismatch on {name}"
            );
            assert_eq!(
                subagent_theme_color(SubagentColor::Orange, &theme),
                theme.subagent_orange,
                "orange mismatch on {name}"
            );
            assert_eq!(
                subagent_theme_color(SubagentColor::Pink, &theme),
                theme.subagent_pink,
                "pink mismatch on {name}"
            );
            assert_eq!(
                subagent_theme_color(SubagentColor::Cyan, &theme),
                theme.subagent_cyan,
                "cyan mismatch on {name}"
            );
        }
    }
}
