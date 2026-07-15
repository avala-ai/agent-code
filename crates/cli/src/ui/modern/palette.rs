//! Ctrl+P command palette for the modern TUI.
//!
//! Fuzzy-ish filter over built-in slash commands (name prefix + description
//! substring). Enter fills the composer with `/cmd `; Esc dismisses.

use super::app::App;

/// In-TUI command palette state.
#[derive(Debug, Clone, Default)]
pub struct CommandPalette {
    /// Filter text (without leading `/`).
    pub query: String,
    /// Highlighted row into the current match list.
    pub selected: usize,
}

impl App {
    /// Open the palette (no-op while a HITL modal owns input).
    pub fn open_command_palette(&mut self) {
        if self.front_modal().is_some() {
            return;
        }
        // Seed from a partial slash already in the composer.
        let seed = if self.input.starts_with('/') {
            self.input.trim_start_matches('/').to_string()
        } else {
            String::new()
        };
        self.command_palette = Some(CommandPalette {
            query: seed,
            selected: 0,
        });
        self.status_message = "command palette · type to filter · Enter select · Esc close".into();
        self.dirty = true;
    }

    pub fn close_command_palette(&mut self) {
        if self.command_palette.take().is_some() {
            self.dirty = true;
        }
    }

    pub fn command_palette_open(&self) -> bool {
        self.command_palette.is_some()
    }

    /// Current filtered matches (name, description).
    pub fn palette_matches(&self) -> Vec<(&'static str, &'static str)> {
        let q = self
            .command_palette
            .as_ref()
            .map(|p| p.query.as_str())
            .unwrap_or("");
        crate::commands::list_slash_for_palette(q)
    }

    pub fn palette_move(&mut self, delta: i32) {
        let Some(p) = self.command_palette.as_mut() else {
            return;
        };
        let n = crate::commands::list_slash_for_palette(&p.query).len();
        if n == 0 {
            p.selected = 0;
            self.dirty = true;
            return;
        }
        let cur = p.selected as i32;
        let next = (cur + delta).rem_euclid(n as i32) as usize;
        p.selected = next;
        self.dirty = true;
    }

    pub fn palette_insert_char(&mut self, c: char) {
        let Some(p) = self.command_palette.as_mut() else {
            return;
        };
        if c.is_control() {
            return;
        }
        p.query.push(c);
        p.selected = 0;
        self.dirty = true;
    }

    pub fn palette_backspace(&mut self) {
        let Some(p) = self.command_palette.as_mut() else {
            return;
        };
        p.query.pop();
        p.selected = 0;
        self.dirty = true;
    }

    /// Accept the highlighted command: fill composer with `/name ` and close.
    pub fn palette_accept(&mut self) {
        let matches = self.palette_matches();
        let idx = self
            .command_palette
            .as_ref()
            .map(|p| p.selected)
            .unwrap_or(0);
        let Some((name, _)) = matches.get(idx).copied() else {
            self.close_command_palette();
            return;
        };
        self.input = format!("/{name} ");
        self.cursor = self.input.len();
        self.history_browse = None;
        self.close_command_palette();
        self.status_message = format!("/{name}");
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::modern::app::App;

    #[test]
    fn open_close_and_accept() {
        let mut app = App::new("m", "/tmp", "s");
        app.open_command_palette();
        assert!(app.command_palette_open());
        // Filter to help
        for c in "hel".chars() {
            app.palette_insert_char(c);
        }
        let matches = app.palette_matches();
        assert!(matches.iter().any(|(n, _)| *n == "help"));
        app.palette_accept();
        assert!(!app.command_palette_open());
        assert!(app.input.starts_with("/help"));
    }

    #[test]
    fn move_wraps() {
        let mut app = App::new("m", "/tmp", "s");
        app.open_command_palette();
        let n = app.palette_matches().len();
        assert!(n > 2);
        app.palette_move(-1);
        assert_eq!(app.command_palette.as_ref().unwrap().selected, n - 1);
        app.palette_move(1);
        assert_eq!(app.command_palette.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn does_not_open_over_permission_modal() {
        use crate::ui::modern::app::{Modal, PendingPermission, Phase};
        use agent_code_lib::tools::PermissionResponse;
        let mut app = App::new("m", "/tmp", "s");
        let (tx, _rx) = std::sync::mpsc::channel::<PermissionResponse>();
        app.modals.push_back(Modal::Permission(PendingPermission {
            name: "Bash".into(),
            description: "run".into(),
            origin: None,
            input_preview: None,
            respond: tx,
        }));
        app.phase = Phase::Permission;
        app.open_command_palette();
        assert!(!app.command_palette_open());
    }
}
