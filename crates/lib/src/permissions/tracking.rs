//! Permission denial tracking.
//!
//! Records which tool calls were denied and why, for reporting
//! to the user and SDK consumers.

use std::collections::VecDeque;

/// A recorded permission denial event.
#[derive(Debug, Clone)]
pub struct DenialRecord {
    /// Tool that was denied.
    pub tool_name: String,
    /// The tool_use ID from the model.
    pub tool_use_id: String,
    /// Reason for denial.
    pub reason: String,
    /// Timestamp of the denial.
    pub timestamp: String,
    /// Summary of what the tool was trying to do.
    pub input_summary: String,
}

/// Tracks permission denials for the current session.
pub struct DenialTracker {
    /// Recent denials (bounded to prevent unbounded growth).
    records: VecDeque<DenialRecord>,
    /// Maximum number of denials to retain.
    max_records: usize,
    /// Total denials this session (even if records were evicted).
    total_denials: usize,
}

impl DenialTracker {
    pub fn new(max_records: usize) -> Self {
        Self {
            records: VecDeque::new(),
            max_records,
            total_denials: 0,
        }
    }

    /// Record a new denial.
    pub fn record(
        &mut self,
        tool_name: &str,
        tool_use_id: &str,
        reason: &str,
        input: &serde_json::Value,
    ) {
        let summary = summarize_input(tool_name, input);

        self.records.push_back(DenialRecord {
            tool_name: tool_name.to_string(),
            tool_use_id: tool_use_id.to_string(),
            reason: reason.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            input_summary: summary,
        });

        self.total_denials += 1;

        // Evict oldest if over limit.
        while self.records.len() > self.max_records {
            self.records.pop_front();
        }
    }

    /// Get all recorded denials.
    pub fn denials(&self) -> &VecDeque<DenialRecord> {
        &self.records
    }

    /// Total denials this session.
    pub fn total(&self) -> usize {
        self.total_denials
    }

    /// Clear all records.
    pub fn clear(&mut self) {
        self.records.clear();
        self.total_denials = 0;
    }

    /// Get denials for a specific tool.
    pub fn denials_for_tool(&self, tool_name: &str) -> Vec<&DenialRecord> {
        self.records
            .iter()
            .filter(|r| r.tool_name == tool_name)
            .collect()
    }

    /// Snapshot denials recorded after `total_before`.
    ///
    /// Returns cloned records so the caller can iterate without
    /// holding the tracker's mutex. If the tracker's `max_records`
    /// limit evicted records that the caller hasn't seen yet, the
    /// returned slice covers only the records still retained —
    /// callers should treat the delta as a lower bound.
    ///
    /// Used by the query engine to emit one `PermissionDenied` hook
    /// event per new denial after each turn.
    pub fn records_since(&self, total_before: usize) -> Vec<DenialRecord> {
        // How many denials arrived since `total_before`.
        let new_count = self.total_denials.saturating_sub(total_before);
        if new_count == 0 {
            return Vec::new();
        }
        // Retained records may be fewer than `new_count` if the
        // tracker evicted older ones. Take the tail of the deque up
        // to `new_count` items.
        let take = new_count.min(self.records.len());
        self.records
            .iter()
            .rev()
            .take(take)
            .rev()
            .cloned()
            .collect()
    }
}

fn summarize_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(100)
            .collect(),
        "FileWrite" | "FileEdit" | "FileRead" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => serde_json::to_string(input)
            .unwrap_or_default()
            .chars()
            .take(100)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tracker() {
        let t = DenialTracker::new(50);
        assert_eq!(t.total(), 0);
        assert!(t.denials().is_empty());
    }

    #[test]
    fn test_record_denial() {
        let mut t = DenialTracker::new(10);
        t.record(
            "Bash",
            "call_1",
            "too dangerous",
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(t.total(), 1);
        assert_eq!(t.denials().len(), 1);
        assert_eq!(t.denials()[0].tool_name, "Bash");
    }

    #[test]
    fn test_denials_for_tool() {
        let mut t = DenialTracker::new(10);
        t.record("Bash", "c1", "reason", &serde_json::json!({}));
        t.record("FileWrite", "c2", "reason", &serde_json::json!({}));
        t.record("Bash", "c3", "reason", &serde_json::json!({}));
        assert_eq!(t.denials_for_tool("Bash").len(), 2);
        assert_eq!(t.denials_for_tool("FileWrite").len(), 1);
        assert_eq!(t.denials_for_tool("Grep").len(), 0);
    }

    #[test]
    fn test_bounded_capacity() {
        let mut t = DenialTracker::new(3);
        for i in 0..5 {
            t.record("Bash", &format!("c{i}"), "r", &serde_json::json!({}));
        }
        assert_eq!(t.total(), 5); // Total tracks all.
        assert_eq!(t.denials().len(), 3); // Deque bounded.
    }

    #[test]
    fn test_clear() {
        let mut t = DenialTracker::new(10);
        t.record("Bash", "c1", "r", &serde_json::json!({}));
        t.clear();
        assert_eq!(t.total(), 0);
        assert!(t.denials().is_empty());
    }

    #[test]
    fn test_input_summary_bash() {
        let mut t = DenialTracker::new(10);
        t.record(
            "Bash",
            "c1",
            "r",
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert!(t.denials()[0].input_summary.contains("rm -rf"));
    }

    #[test]
    fn test_input_summary_file() {
        let mut t = DenialTracker::new(10);
        t.record(
            "FileWrite",
            "c1",
            "r",
            &serde_json::json!({"file_path": "/etc/passwd"}),
        );
        assert!(t.denials()[0].input_summary.contains("/etc/passwd"));
    }

    // ---- records_since ----

    #[test]
    fn records_since_zero_returns_all_retained() {
        let mut t = DenialTracker::new(10);
        t.record("Bash", "c1", "r1", &serde_json::json!({}));
        t.record("Bash", "c2", "r2", &serde_json::json!({}));
        let got = t.records_since(0);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].tool_use_id, "c1");
        assert_eq!(got[1].tool_use_id, "c2");
    }

    #[test]
    fn records_since_uses_total_not_retained_length() {
        // `records_since` is indexed on total_denials, not on the
        // retained-queue length. That matters because callers pass
        // the tracker's `total()` value from the previous turn.
        let mut t = DenialTracker::new(10);
        t.record("Bash", "c1", "r1", &serde_json::json!({}));
        t.record("Bash", "c2", "r2", &serde_json::json!({}));
        let after_first_turn = t.total();
        t.record("Bash", "c3", "r3", &serde_json::json!({}));
        let new_records = t.records_since(after_first_turn);
        assert_eq!(new_records.len(), 1);
        assert_eq!(new_records[0].tool_use_id, "c3");
    }

    #[test]
    fn records_since_when_no_new_denials_is_empty() {
        let mut t = DenialTracker::new(10);
        t.record("Bash", "c1", "r1", &serde_json::json!({}));
        let total = t.total();
        assert!(t.records_since(total).is_empty());
        // Asking for a total past the actual count should also be
        // empty rather than panic.
        assert!(t.records_since(total + 100).is_empty());
    }

    #[test]
    fn records_since_clamps_to_retained_when_evicted() {
        // Ring-buffer eviction: tracker has max_records=2, 5 denials
        // arrive after checkpoint. We can only return the latest 2.
        let mut t = DenialTracker::new(2);
        let checkpoint = t.total();
        for i in 0..5 {
            t.record("Bash", &format!("c{i}"), "r", &serde_json::json!({}));
        }
        let new_records = t.records_since(checkpoint);
        assert_eq!(
            new_records.len(),
            2,
            "should clamp to retained deque length"
        );
        // Latest two are c3, c4.
        assert_eq!(new_records[0].tool_use_id, "c3");
        assert_eq!(new_records[1].tool_use_id, "c4");
    }
}
