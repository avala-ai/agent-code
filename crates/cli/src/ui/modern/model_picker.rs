//! In-TUI model picker (Ctrl+M / `/model`).
//!
//! Lists provider catalog entries with filter + optional effort sub-menu
//! (Grok Build product motion: pick model, Tab for reasoning effort).

use super::app::{App, EFFORT_LEVELS, PendingModelAction};

/// Overlay state for the model picker.
#[derive(Debug, Clone)]
pub struct ModelPicker {
    /// Filter text over model ids / descriptions.
    pub query: String,
    /// Highlighted row into the filtered list (model phase) or effort list.
    pub selected: usize,
    /// Full catalog: (id, description).
    pub entries: Vec<(String, String)>,
    /// Model active when the picker opened.
    pub current: String,
    /// When true, the list shows effort levels for the highlighted model.
    pub effort_phase: bool,
    /// Selected effort row.
    pub effort_selected: usize,
}

impl ModelPicker {
    /// Filtered model rows matching `query`.
    pub fn filtered(&self) -> Vec<(usize, &str, &str)> {
        let q = self.query.to_ascii_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, (id, desc))| {
                if q.is_empty() {
                    return true;
                }
                id.to_ascii_lowercase().contains(&q)
                    || desc.to_ascii_lowercase().contains(&q)
            })
            .map(|(i, (id, desc))| (i, id.as_str(), desc.as_str()))
            .collect()
    }
}

impl App {
    /// Request opening the model picker (run loop fills catalog via pending Show).
    pub fn request_model_picker(&mut self) {
        if self.front_modal().is_some() {
            return;
        }
        self.pending_model = Some(PendingModelAction::Show);
        self.dirty = true;
    }

    pub fn model_picker_move(&mut self, delta: i32) {
        let Some(p) = self.model_picker.as_mut() else {
            return;
        };
        if p.effort_phase {
            let n = EFFORT_LEVELS.len() as i32;
            let cur = p.effort_selected as i32;
            p.effort_selected = (cur + delta).rem_euclid(n) as usize;
        } else {
            let n = p.filtered().len() as i32;
            if n == 0 {
                p.selected = 0;
            } else {
                let cur = p.selected as i32;
                p.selected = (cur + delta).rem_euclid(n) as usize;
            }
        }
        self.dirty = true;
    }

    pub fn model_picker_insert_char(&mut self, c: char) {
        let Some(p) = self.model_picker.as_mut() else {
            return;
        };
        if p.effort_phase || c.is_control() {
            return;
        }
        p.query.push(c);
        p.selected = 0;
        self.dirty = true;
    }

    pub fn model_picker_backspace(&mut self) {
        let Some(p) = self.model_picker.as_mut() else {
            return;
        };
        if p.effort_phase {
            p.effort_phase = false;
            self.status_message = "model picker · type to filter · Enter select · Tab effort".into();
        } else {
            p.query.pop();
            p.selected = 0;
        }
        self.dirty = true;
    }

    /// Enter effort sub-menu for the highlighted model (Tab).
    pub fn model_picker_enter_effort(&mut self) {
        let current_effort = self.effort.clone();
        let Some(p) = self.model_picker.as_mut() else {
            return;
        };
        if p.effort_phase {
            return;
        }
        let filtered = p.filtered();
        if filtered.is_empty() {
            return;
        }
        p.effort_phase = true;
        p.effort_selected = EFFORT_LEVELS
            .iter()
            .position(|l| current_effort.as_deref() == Some(*l))
            .unwrap_or(2); // default highlight medium
        self.status_message = "effort · ↑/↓ · Enter apply · Esc/Backspace back".into();
        self.dirty = true;
    }

    /// Accept the current picker selection.
    pub fn model_picker_accept(&mut self) {
        let Some(p) = self.model_picker.clone() else {
            return;
        };
        if p.effort_phase {
            let filtered = p.filtered();
            let Some((_, model_id, _)) = filtered.get(p.selected).copied() else {
                self.close_model_picker();
                return;
            };
            let effort = EFFORT_LEVELS
                .get(p.effort_selected)
                .copied()
                .unwrap_or("medium")
                .to_string();
            let effort = if effort == "max" {
                "xhigh".to_string()
            } else {
                effort
            };
            self.close_model_picker();
            self.pending_model = Some(PendingModelAction::Set {
                model: model_id.to_string(),
                effort: Some(effort),
            });
            return;
        }
        let filtered = p.filtered();
        let Some((_, model_id, _)) = filtered.get(p.selected).copied() else {
            self.close_model_picker();
            return;
        };
        self.close_model_picker();
        self.pending_model = Some(PendingModelAction::Set {
            model: model_id.to_string(),
            effort: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::modern::app::App;

    #[test]
    fn open_filter_and_accept() {
        let mut app = App::new("m", "/tmp", "s");
        app.open_model_picker(
            "gpt-4o",
            vec![
                ("gpt-4o".into(), "fast".into()),
                ("o3".into(), "reason".into()),
            ],
        );
        assert!(app.model_picker_open());
        app.model_picker_insert_char('o');
        app.model_picker_insert_char('3');
        let filtered = app.model_picker.as_ref().unwrap().filtered();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1, "o3");
        app.model_picker_accept();
        assert!(!app.model_picker_open());
        assert_eq!(
            app.pending_model,
            Some(PendingModelAction::Set {
                model: "o3".into(),
                effort: None
            })
        );
    }

    #[test]
    fn effort_phase_sets_both() {
        let mut app = App::new("m", "/tmp", "s");
        app.open_model_picker(
            "o3",
            vec![("o3".into(), "reason".into())],
        );
        app.model_picker_enter_effort();
        assert!(app.model_picker.as_ref().unwrap().effort_phase);
        // Select "high" (index of high in EFFORT_LEVELS)
        let high_idx = EFFORT_LEVELS.iter().position(|l| *l == "high").unwrap();
        app.model_picker.as_mut().unwrap().effort_selected = high_idx;
        app.model_picker_accept();
        assert_eq!(
            app.pending_model,
            Some(PendingModelAction::Set {
                model: "o3".into(),
                effort: Some("high".into())
            })
        );
    }
}
