//! Input history management
//!
//! Tracks user input history for command recall and search.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An item in the input history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputHistoryItem {
    /// The input content
    pub content: String,
    /// When this input was last used
    pub timestamp: DateTime<Utc>,
    /// Number of times this input was used
    pub use_count: usize,
    /// Sequence number for stable ordering (higher = newer)
    #[serde(default)]
    pub sequence: u64,
}

/// Input history manager
#[derive(Debug, Default)]
pub struct InputHistory {
    items: Vec<InputHistoryItem>,
    max_size: usize,
    next_sequence: u64,
}

impl InputHistory {
    /// Create a new input history with default max size
    #[must_use]
    pub const fn new() -> Self {
        Self { items: Vec::new(), max_size: 1000, next_sequence: 0 }
    }

    /// Create with custom max size
    #[must_use]
    pub const fn with_max_size(max_size: usize) -> Self {
        Self { items: Vec::new(), max_size, next_sequence: 0 }
    }

    /// Add an input to history
    pub fn add(&mut self, input: &str) {
        if input.trim().is_empty() {
            return;
        }

        let sequence = self.next_sequence;
        self.next_sequence += 1;

        // Check if this input already exists
        if let Some(item) = self.items.iter_mut().find(|i| i.content == input) {
            item.use_count += 1;
            item.timestamp = Utc::now();
            item.sequence = sequence;
        } else {
            self.items.push(InputHistoryItem {
                content: input.to_string(),
                timestamp: Utc::now(),
                use_count: 1,
                sequence,
            });
        }

        // Enforce max size
        if self.items.len() > self.max_size {
            self.items.sort_by(|a, b| a.sequence.cmp(&b.sequence));
            self.items.remove(0);
        }
    }

    /// Get recent inputs (newest first)
    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<&str> {
        let mut sorted: Vec<_> = self.items.iter().collect();
        sorted.sort_by(|a, b| b.sequence.cmp(&a.sequence));
        sorted.into_iter().take(limit).map(|i| i.content.as_str()).collect()
    }

    /// Search history by query
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&InputHistoryItem> {
        let query_lower = query.to_lowercase();
        let mut results: Vec<_> =
            self.items.iter().filter(|i| i.content.to_lowercase().contains(&query_lower)).collect();
        results.sort_by(|a, b| {
            b.use_count.cmp(&a.use_count).then_with(|| b.timestamp.cmp(&a.timestamp))
        });
        results
    }

    /// Get all items
    #[must_use]
    pub fn items(&self) -> &[InputHistoryItem] {
        &self.items
    }

    /// Load items (for persistence)
    pub fn load(&mut self, items: Vec<InputHistoryItem>) {
        self.items = items;
        self.next_sequence = self.items.iter().map(|i| i.sequence).max().unwrap_or(0) + 1;
        while self.items.len() > self.max_size {
            self.items.sort_by(|a, b| a.sequence.cmp(&b.sequence));
            self.items.remove(0);
        }
    }

    /// Clear all history
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Get the number of items
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if history is empty
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_recent() {
        let mut history = InputHistory::new();
        history.add("first");
        history.add("second");
        history.add("third");

        let recent = history.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0], "third");
        assert_eq!(recent[1], "second");
    }

    #[test]
    fn test_duplicate_increases_count() {
        let mut history = InputHistory::new();
        history.add("test");
        history.add("test");
        history.add("test");

        assert_eq!(history.len(), 1);
        assert_eq!(history.items[0].use_count, 3);
    }

    #[test]
    fn test_search() {
        let mut history = InputHistory::new();
        history.add("hello world");
        history.add("hello rust");
        history.add("goodbye world");

        let results = history.search("hello");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_max_size() {
        let mut history = InputHistory::with_max_size(3);
        history.add("one");
        history.add("two");
        history.add("three");
        history.add("four");

        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_empty_input_ignored() {
        let mut history = InputHistory::new();
        history.add("");
        history.add("   ");

        assert!(history.is_empty());
    }
}
