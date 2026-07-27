//! In-TUI theme picker (`/theme`).
//!
//! Lists every theme the registry knows about — `auto`, the standard
//! palettes and any user-defined TOML palettes — with a filter box.
//!
//! The picker previews **live**: moving the highlight re-inits the global
//! theme, so the whole TUI repaints in the candidate palette while you
//! browse. Esc restores whatever was active when the picker opened, so
//! browsing is free. That is the whole point of an in-TUI picker: a list
//! of names you cannot see is barely better than editing the config file.

use super::app::App;
use crate::ui::theme;

/// Overlay state for the theme picker.
#[derive(Debug, Clone)]
pub struct ThemePicker {
    /// Filter text over theme ids / labels.
    pub query: String,
    /// Highlighted row into the filtered list.
    pub selected: usize,
    /// Full registry: (id, human label).
    pub entries: Vec<(String, String)>,
    /// Theme id active when the picker opened. Esc restores it.
    pub original: String,
}

impl ThemePicker {
    /// Filtered theme rows matching `query`.
    pub fn filtered(&self) -> Vec<(usize, &str, &str)> {
        let q = self.query.to_ascii_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, (id, label))| {
                if q.is_empty() {
                    return true;
                }
                id.to_ascii_lowercase().contains(&q) || label.to_ascii_lowercase().contains(&q)
            })
            .map(|(i, (id, label))| (i, id.as_str(), label.as_str()))
            .collect()
    }

    /// Id under the highlight, if the filter matched anything.
    pub fn highlighted(&self) -> Option<String> {
        self.filtered()
            .get(self.selected)
            .map(|(_, id, _)| (*id).to_string())
    }
}

impl App {
    /// Open the theme picker (`/theme`).
    pub fn open_theme_picker(&mut self) {
        if self.front_modal().is_some() {
            return;
        }
        self.command_palette = None;
        self.show_shortcuts = false;
        let entries = theme::Theme::all_options();
        let original = self.theme_name.clone();
        let selected = entries
            .iter()
            .position(|(id, _)| *id == original)
            .unwrap_or(0);
        self.theme_picker = Some(ThemePicker {
            query: String::new(),
            selected,
            entries,
            original,
        });
        // Preview whatever is already under the highlight so the first
        // frame agrees with the selection marker.
        self.preview_highlighted_theme();
        self.status_message =
            "theme picker · type to filter · ↑↓ preview · Enter keep · Esc revert".into();
        self.dirty = true;
    }

    pub fn close_theme_picker(&mut self) {
        if self.theme_picker.take().is_some() {
            self.dirty = true;
        }
    }

    pub fn theme_picker_open(&self) -> bool {
        self.theme_picker.is_some()
    }

    pub fn theme_picker_move(&mut self, delta: i32) {
        let Some(p) = self.theme_picker.as_mut() else {
            return;
        };
        let n = p.filtered().len() as i32;
        if n == 0 {
            p.selected = 0;
        } else {
            let cur = p.selected as i32;
            p.selected = (cur + delta).rem_euclid(n) as usize;
        }
        self.preview_highlighted_theme();
        self.dirty = true;
    }

    pub fn theme_picker_insert_char(&mut self, c: char) {
        let Some(p) = self.theme_picker.as_mut() else {
            return;
        };
        if c.is_control() {
            return;
        }
        p.query.push(c);
        p.selected = 0;
        self.preview_highlighted_theme();
        self.dirty = true;
    }

    pub fn theme_picker_backspace(&mut self) {
        let Some(p) = self.theme_picker.as_mut() else {
            return;
        };
        p.query.pop();
        p.selected = 0;
        self.preview_highlighted_theme();
        self.dirty = true;
    }

    /// Keep the previewed theme and close (Enter).
    pub fn theme_picker_accept(&mut self) {
        let Some(p) = self.theme_picker.as_ref() else {
            return;
        };
        let Some(id) = p.highlighted() else {
            // Nothing matched the filter: treat Enter as a cancel rather
            // than silently keeping a preview the user never selected.
            self.theme_picker_cancel();
            return;
        };
        self.close_theme_picker();
        self.set_theme(&id);
        self.status_message = format!("theme → {id}");
        self.transcript
            .push(super::app::TranscriptItem::System(format!(
                "theme → {id}  (/theme to change · /minimal · /fullscreen for layout)"
            )));
    }

    /// Restore the theme that was active when the picker opened (Esc).
    pub fn theme_picker_cancel(&mut self) {
        let Some(p) = self.theme_picker.as_ref() else {
            return;
        };
        let original = p.original.clone();
        self.close_theme_picker();
        // Restore, do not commit: the config already says `original`, so
        // a cancel must not queue a config write.
        self.apply_theme(&original);
        self.theme_name = original;
        self.status_message = "theme unchanged".into();
    }

    /// Apply `id` as the live theme and remember it as the session's
    /// configured name. `pending_theme` asks the run loop to mirror it
    /// into the engine config so `/config` and `/color` agree.
    pub fn set_theme(&mut self, id: &str) {
        self.apply_theme(id);
        self.theme_name = id.to_string();
        self.pending_theme = Some(id.to_string());
    }

    /// Repaint in `id` without touching what the session considers its
    /// configured theme. Both the live preview and the Esc restore go
    /// through here, so neither can leave a config write queued.
    ///
    /// The layout cache holds already-styled lines, so it has to be
    /// dropped or the transcript keeps rendering in the old palette while
    /// the chrome switches — a half-repainted screen.
    fn apply_theme(&mut self, id: &str) {
        let resolved = theme::resolve_theme(id);
        theme::init_with_options(&resolved, id, self.inherit_fg);
        self.layout.invalidate();
        self.dirty = true;
    }

    /// Repaint in the highlighted candidate without committing to it.
    fn preview_highlighted_theme(&mut self) {
        let Some(id) = self.theme_picker.as_ref().and_then(|p| p.highlighted()) else {
            return;
        };
        self.apply_theme(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test here mutates the process-global theme, so each holds
    /// [`theme::test_lock`] for its whole body.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        theme::test_lock()
    }

    fn app() -> App {
        let mut app = App::new("m", "/tmp", "s");
        app.set_theme("one-dark");
        app.pending_theme = None;
        app
    }

    #[test]
    fn opens_highlighting_the_active_theme() {
        let _g = guard();
        let mut a = app();
        a.open_theme_picker();
        assert!(a.theme_picker_open());
        let p = a.theme_picker.as_ref().unwrap();
        assert_eq!(p.original, "one-dark");
        assert_eq!(p.highlighted().as_deref(), Some("one-dark"));
    }

    #[test]
    fn filter_narrows_and_enter_keeps_the_choice() {
        let _g = guard();
        let mut a = app();
        a.open_theme_picker();
        for c in "gruv".chars() {
            a.theme_picker_insert_char(c);
        }
        let hit = a.theme_picker.as_ref().unwrap().highlighted().unwrap();
        assert!(hit.contains("gruvbox"), "unexpected match: {hit}");
        a.theme_picker_accept();
        assert!(!a.theme_picker_open());
        assert_eq!(a.theme_name, hit);
        assert_eq!(a.pending_theme.as_deref(), Some(hit.as_str()));
    }

    #[test]
    fn escape_reverts_the_live_preview() {
        let _g = guard();
        let mut a = app();
        a.open_theme_picker();
        a.theme_picker_move(1);
        a.theme_picker_move(1);
        // Browsing repainted, but nothing was committed.
        assert_eq!(a.theme_name, "one-dark");
        assert!(a.pending_theme.is_none(), "a preview must not persist");
        a.theme_picker_cancel();
        assert!(!a.theme_picker_open());
        assert_eq!(a.theme_name, "one-dark");
        assert_eq!(
            crate::ui::theme::current().accent,
            crate::ui::theme::Theme::from_name("one-dark").accent,
            "Esc must restore the palette that was active on open"
        );
    }

    #[test]
    fn enter_with_no_match_cancels_instead_of_committing_a_preview() {
        let _g = guard();
        let mut a = app();
        a.open_theme_picker();
        for c in "zzzz".chars() {
            a.theme_picker_insert_char(c);
        }
        assert!(a.theme_picker.as_ref().unwrap().filtered().is_empty());
        a.theme_picker_accept();
        assert!(!a.theme_picker_open());
        assert_eq!(a.theme_name, "one-dark");
        assert!(a.pending_theme.is_none());
    }

    #[test]
    fn every_registry_theme_is_offered() {
        let _g = guard();
        let mut a = app();
        a.open_theme_picker();
        let offered: Vec<String> = a
            .theme_picker
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        // The picker and `/color` must agree on the catalog, or one of
        // them is advertising themes the other rejects.
        assert_eq!(offered, crate::ui::theme::Theme::all_names());
        assert!(offered.contains(&"auto".to_string()));
    }
}
