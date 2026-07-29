//! Lightweight fuzzy matching for palette / completion filtering.
//!
//! Subsequence match with a simple score: earlier matches and contiguous
//! runs rank higher. No external crate — keeps the CLI hermetic and
//! avoids another matcher convention.
//!
//! Used by the command palette today; completion dropdown (#560) will
//! share the same scorer so slash, path, and shell providers stay
//! consistent.

/// Score of how well `query` fuzzy-matches `candidate` (case-insensitive).
///
/// Returns `None` when the query is not a subsequence of the candidate.
/// Higher is better. Empty query matches everything with score 0.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_ascii_lowercase().chars().collect();
    let c: Vec<char> = candidate.to_ascii_lowercase().chars().collect();
    if q.len() > c.len() {
        return None;
    }

    let mut score: i64 = 0;
    let mut qi = 0usize;
    let mut prev_match: Option<usize> = None;
    let mut run = 0i64;

    for (ci, &ch) in c.iter().enumerate() {
        if qi < q.len() && ch == q[qi] {
            // Prefer earlier matches.
            score += 1_000 - (ci as i64).min(900);
            if let Some(p) = prev_match {
                if ci == p + 1 {
                    run += 1;
                    score += 50 * run; // contiguous bonus
                } else {
                    run = 0;
                }
            }
            // Bonus for matching at a word boundary.
            if ci == 0
                || matches!(
                    c.get(ci.wrapping_sub(1)),
                    Some('-' | '_' | '/' | ' ' | '.' | ':')
                )
            {
                score += 30;
            }
            prev_match = Some(ci);
            qi += 1;
            if qi == q.len() {
                // Prefer shorter candidates when equally matched.
                score += 100 - (c.len() as i64 - q.len() as i64).min(99);
                return Some(score);
            }
        }
    }
    None
}

/// Whether `query` is a case-insensitive subsequence of `candidate`.
pub fn fuzzy_match(query: &str, candidate: &str) -> bool {
    fuzzy_score(query, candidate).is_some()
}

/// Filter and rank `items` by fuzzy score against `query`.
///
/// `key` extracts the string to match. Results are sorted best-first;
/// empty query returns items in the original order.
pub fn fuzzy_rank<T, F>(query: &str, items: impl IntoIterator<Item = T>, key: F) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    let q = query.trim();
    if q.is_empty() {
        return items.into_iter().collect();
    }
    let mut scored: Vec<(i64, usize, T)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, item)| fuzzy_score(q, key(&item)).map(|s| (s, i, item)))
        .collect();
    // Higher score first; stable on original index.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches() {
        assert_eq!(fuzzy_score("", "help"), Some(0));
        assert!(fuzzy_match("", "anything"));
    }

    #[test]
    fn subsequence_required() {
        assert!(fuzzy_match("hlp", "help"));
        assert!(!fuzzy_match("xyz", "help"));
        assert!(fuzzy_match("mdl", "model"));
    }

    #[test]
    fn contiguous_outranks_scattered() {
        let tight = fuzzy_score("hel", "help").unwrap();
        let loose = fuzzy_score("hel", "h-e-l-p").unwrap();
        assert!(
            tight > loose,
            "contiguous run should score higher: {tight} vs {loose}"
        );
    }

    #[test]
    fn rank_orders_best_first() {
        let items = vec!["status", "sessions", "session", "help"];
        let ranked = fuzzy_rank("sess", items, |s| s);
        assert_eq!(ranked[0], "session");
        assert!(ranked.contains(&"sessions"));
        assert!(!ranked.contains(&"help"));
    }

    #[test]
    fn prefix_still_matches() {
        assert!(fuzzy_match("hel", "help"));
        let s = fuzzy_score("help", "help").unwrap();
        assert!(s > 0);
    }
}
