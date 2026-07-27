//! Vi editing mode for the composer.
//!
//! `/vim` set `ui.edit_mode = "vi"` and printed "Takes effect on next
//! session", but the modern TUI never read the setting — it took effect
//! in no session at all. The command dates from the rustyline REPL,
//! which had vi bindings natively; the REPL was removed and the config
//! key outlived it.
//!
//! The motion set is deliberately small. A partial vi that quietly does
//! the wrong thing on an unimplemented key is worse than one whose edges
//! are obvious, so unknown keys in normal mode do nothing rather than
//! falling through to the global chord handler and firing a shortcut the
//! user did not intend.

use super::app::{App, ComposerMode};

/// Result of offering a key to normal mode.
#[derive(Debug, PartialEq, Eq)]
pub enum ViOutcome {
    /// Normal mode handled it; stop processing.
    Consumed,
    /// Not a normal-mode key; the caller may keep going.
    Fallthrough,
}

impl App {
    /// True when the composer is in vi normal mode.
    pub fn in_normal_mode(&self) -> bool {
        self.vi_mode && self.composer_mode == ComposerMode::Normal
    }

    /// Leave insert mode. Vi puts the cursor on the last typed character
    /// rather than after it.
    pub fn vi_enter_normal(&mut self) {
        if !self.vi_mode {
            return;
        }
        self.composer_mode = ComposerMode::Normal;
        self.cursor = prev_boundary(&self.input, self.cursor);
        self.dirty = true;
    }

    pub fn vi_enter_insert(&mut self) {
        self.composer_mode = ComposerMode::Insert;
        self.dirty = true;
    }

    /// Handle a character in normal mode.
    pub fn vi_normal_key(&mut self, c: char) -> ViOutcome {
        // A pending `d` waits for its motion; `dd` is the only one.
        if self.vi_pending_d {
            self.vi_pending_d = false;
            if c == 'd' {
                self.input.clear();
                self.cursor = 0;
                self.dirty = true;
            }
            return ViOutcome::Consumed;
        }

        match c {
            'h' => self.cursor = prev_boundary(&self.input, self.cursor),
            'l' => self.cursor = next_boundary(&self.input, self.cursor),
            '0' => self.cursor = 0,
            '$' => self.cursor = prev_boundary(&self.input, self.input.len()),
            'w' => self.cursor = word_forward(&self.input, self.cursor),
            'b' => self.cursor = word_back(&self.input, self.cursor),
            'i' => self.vi_enter_insert(),
            'a' => {
                self.cursor = next_boundary(&self.input, self.cursor);
                self.vi_enter_insert();
            }
            'I' => {
                self.cursor = 0;
                self.vi_enter_insert();
            }
            'A' => {
                self.cursor = self.input.len();
                self.vi_enter_insert();
            }
            'x' => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                    // Deleting the last character leaves the cursor on
                    // the new last one, as vi does.
                    if self.cursor >= self.input.len() {
                        self.cursor = prev_boundary(&self.input, self.input.len());
                    }
                }
            }
            'D' => self.input.truncate(self.cursor),
            'C' => {
                self.input.truncate(self.cursor);
                self.vi_enter_insert();
            }
            'd' => self.vi_pending_d = true,
            // Unknown normal-mode key: swallow it. Falling through would
            // run a global chord the user never meant to trigger.
            _ => {}
        }
        self.dirty = true;
        ViOutcome::Consumed
    }
}

fn prev_boundary(s: &str, from: usize) -> usize {
    if from == 0 {
        return 0;
    }
    let mut i = from - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_boundary(s: &str, from: usize) -> usize {
    if from >= s.len() {
        return s.len();
    }
    let mut i = from + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Start of the next word, vi's `w`.
fn word_forward(s: &str, from: usize) -> usize {
    let bytes: Vec<(usize, char)> = s.char_indices().collect();
    let mut idx = bytes
        .iter()
        .position(|(i, _)| *i >= from)
        .unwrap_or(bytes.len());
    // Skip the current word, then the whitespace after it.
    while idx < bytes.len() && !bytes[idx].1.is_whitespace() {
        idx += 1;
    }
    while idx < bytes.len() && bytes[idx].1.is_whitespace() {
        idx += 1;
    }
    bytes.get(idx).map(|(i, _)| *i).unwrap_or(s.len())
}

/// Start of the previous word, vi's `b`.
fn word_back(s: &str, from: usize) -> usize {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut idx = chars
        .iter()
        .position(|(i, _)| *i >= from)
        .unwrap_or(chars.len());
    if idx == 0 {
        return 0;
    }
    idx -= 1;
    while idx > 0 && chars[idx].1.is_whitespace() {
        idx -= 1;
    }
    while idx > 0 && !chars[idx - 1].1.is_whitespace() {
        idx -= 1;
    }
    chars.get(idx).map(|(i, _)| *i).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vi_app(text: &str) -> App {
        let mut app = App::new("m", "/tmp", "s");
        app.vi_mode = true;
        app.input = text.to_string();
        app.cursor = text.len();
        app.composer_mode = ComposerMode::Normal;
        app
    }

    #[test]
    fn motions_move_the_cursor() {
        let mut a = vi_app("hello world");
        a.cursor = 0;
        a.vi_normal_key('l');
        assert_eq!(a.cursor, 1);
        a.vi_normal_key('h');
        assert_eq!(a.cursor, 0);
        a.vi_normal_key('$');
        assert_eq!(a.cursor, 10, "$ sits on the last char, not past it");
        a.vi_normal_key('0');
        assert_eq!(a.cursor, 0);
        a.vi_normal_key('w');
        assert_eq!(a.cursor, 6, "w should reach the start of `world`");
        a.vi_normal_key('b');
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn insert_entries_position_the_cursor_correctly() {
        let mut a = vi_app("abc");
        a.cursor = 1;
        a.vi_normal_key('i');
        assert_eq!((a.cursor, a.composer_mode), (1, ComposerMode::Insert));

        let mut a = vi_app("abc");
        a.cursor = 1;
        a.vi_normal_key('a');
        assert_eq!(a.cursor, 2, "`a` appends after the cursor");

        let mut a = vi_app("abc");
        a.vi_normal_key('I');
        assert_eq!(a.cursor, 0);

        let mut a = vi_app("abc");
        a.cursor = 0;
        a.vi_normal_key('A');
        assert_eq!(a.cursor, 3, "`A` appends at end of line");
    }

    #[test]
    fn edits_change_the_line() {
        let mut a = vi_app("abc");
        a.cursor = 0;
        a.vi_normal_key('x');
        assert_eq!(a.input, "bc");

        let mut a = vi_app("hello world");
        a.cursor = 5;
        a.vi_normal_key('D');
        assert_eq!(a.input, "hello");

        let mut a = vi_app("hello world");
        a.cursor = 5;
        a.vi_normal_key('C');
        assert_eq!(a.input, "hello");
        assert_eq!(a.composer_mode, ComposerMode::Insert, "C enters insert");
    }

    /// `d` alone must not delete; only `dd` clears the line.
    #[test]
    fn d_waits_for_its_motion() {
        let mut a = vi_app("hello");
        a.vi_normal_key('d');
        assert_eq!(a.input, "hello", "a lone `d` deleted something");
        a.vi_normal_key('d');
        assert_eq!(a.input, "", "dd should clear the line");

        // `d` followed by an unknown motion cancels rather than deleting.
        let mut a = vi_app("hello");
        a.vi_normal_key('d');
        a.vi_normal_key('z');
        assert_eq!(a.input, "hello");
    }

    /// An unhandled normal-mode key must be swallowed, not passed on —
    /// otherwise `q` or `t` would fire a global chord mid-edit.
    #[test]
    fn unknown_normal_keys_are_consumed() {
        let mut a = vi_app("hello");
        assert_eq!(a.vi_normal_key('q'), ViOutcome::Consumed);
        assert_eq!(a.vi_normal_key('Z'), ViOutcome::Consumed);
        assert_eq!(a.input, "hello");
    }

    #[test]
    fn leaving_insert_pulls_the_cursor_back_one() {
        let mut a = vi_app("abc");
        a.composer_mode = ComposerMode::Insert;
        a.cursor = 3;
        a.vi_enter_normal();
        assert_eq!(a.composer_mode, ComposerMode::Normal);
        assert_eq!(a.cursor, 2, "vi leaves the cursor on the last char");
    }

    /// Without vi mode enabled the composer must behave exactly as before.
    #[test]
    fn normal_mode_is_inert_when_vi_is_off() {
        let mut app = App::new("m", "/tmp", "s");
        app.input = "abc".into();
        app.cursor = 3;
        app.vi_enter_normal();
        assert!(!app.in_normal_mode());
        assert_eq!(app.cursor, 3, "cursor moved with vi mode off");
    }

    /// Multi-byte text must not panic or split a character.
    #[test]
    fn motions_respect_char_boundaries() {
        let mut a = vi_app("héllo wörld");
        a.cursor = 0;
        for _ in 0..12 {
            a.vi_normal_key('l');
        }
        assert!(a.input.is_char_boundary(a.cursor));
        for _ in 0..12 {
            a.vi_normal_key('h');
        }
        assert_eq!(a.cursor, 0);
        a.vi_normal_key('w');
        assert!(a.input.is_char_boundary(a.cursor));
    }
}
