use std::collections::HashMap;
use std::time::Instant;
use nucleo_matcher::{Matcher, Config as NucleoConfig, Utf32Str, pattern::{Pattern, AtomKind, CaseMatching, Normalization}};

/// Lightweight wrapper around nucleo-matcher for fuzzy string scoring.
pub struct FuzzyMatch {
    matcher: Matcher,
    char_buf: Vec<char>,
}

impl Default for FuzzyMatch {
    fn default() -> Self {
        Self::new()
    }
}

impl FuzzyMatch {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(NucleoConfig::DEFAULT.match_paths()),
            char_buf: Vec::new(),
        }
    }

    /// Score a list of (name, description) pairs against a query string.
    /// Returns indices into the input slice, sorted by score descending.
    pub fn score(
        &mut self,
        query: &str,
        items: &[(String, String)],
    ) -> Vec<usize> {
        if query.is_empty() {
            return (0..items.len()).collect();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut scored: Vec<(u32, usize)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, (name, _desc))| {
                let haystack = Utf32Str::new(name, &mut self.char_buf);
                let score = pattern.score(haystack, &mut self.matcher);
                score.map(|s| (s, i))
            })
            .collect();

        scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
        scored.into_iter().map(|(_, i)| i).collect()
    }
}

/// Exponential decay MRU ranker.
/// Keys on `stable_id: String` — semantic identity that survives node allocation cycles.
pub struct MruRanker {
    entries: HashMap<String, MruEntry>,
    decay_per_use: f64,
}

struct MruEntry {
    weight: f64,
    #[allow(dead_code)]
    last_used: Instant,
}

impl Default for MruRanker {
    fn default() -> Self {
        Self::new()
    }
}

impl MruRanker {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            decay_per_use: 0.95,
        }
    }

    /// Record a command execution. Resets the entry's weight and decays all others.
    pub fn record(&mut self, stable_id: &str) {
        let now = Instant::now();
        // Decay all existing entries
        for entry in self.entries.values_mut() {
            entry.weight *= self.decay_per_use;
        }
        // Set or reset the entry
        self.entries
            .entry(stable_id.to_string())
            .and_modify(|e| {
                e.weight = 1.0;
                e.last_used = now;
            })
            .or_insert(MruEntry {
                weight: 1.0,
                last_used: now,
            });
    }

    /// Get the MRU weight for a stable_id (0.0 if never used).
    pub fn weight(&self, stable_id: &str) -> f64 {
        self.entries.get(stable_id).map_or(0.0, |e| e.weight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_empty_query_returns_all() {
        let mut fmatch = FuzzyMatch::new();
        let items = vec![
            ("New Window".to_string(), String::new()),
            ("Close Window".to_string(), String::new()),
        ];
        let results = fmatch.score("", &items);
        assert_eq!(results, vec![0, 1]);
    }

    #[test]
    fn fuzzy_matching_prefix() {
        let mut fmatch = FuzzyMatch::new();
        let items = vec![
            ("New Window".to_string(), String::new()),
            ("Close Window".to_string(), String::new()),
        ];
        let results = fmatch.score("new", &items);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], 0);
    }

    #[test]
    fn fuzzy_no_match_returns_empty() {
        let mut fmatch = FuzzyMatch::new();
        let items = vec![
            ("New Window".to_string(), String::new()),
            ("Close Window".to_string(), String::new()),
        ];
        let results = fmatch.score("zzzzz", &items);
        assert!(results.is_empty());
    }

    #[test]
    fn mru_weight_starts_at_zero() {
        let ranker = MruRanker::new();
        assert_eq!(ranker.weight("nonexistent"), 0.0);
    }

    #[test]
    fn mru_record_sets_weight_to_one() {
        let mut ranker = MruRanker::new();
        ranker.record("test:cmd");
        assert_eq!(ranker.weight("test:cmd"), 1.0);
    }

    #[test]
    fn mru_decay_on_new_record() {
        let mut ranker = MruRanker::new();
        ranker.record("a");
        ranker.record("b");
        // "a" should have been decayed
        assert!(ranker.weight("a") < 1.0);
        assert!(ranker.weight("a") > 0.0);
        assert_eq!(ranker.weight("b"), 1.0);
    }

    #[test]
    fn mru_keys_by_stable_id_not_node_id() {
        let mut ranker = MruRanker::new();
        ranker.record("git:commit");
        assert_eq!(ranker.weight("git:commit"), 1.0);
        // Same stable_id always maps to same weight regardless of allocation
        assert_eq!(ranker.weight("git:commit"), 1.0);
    }
}
