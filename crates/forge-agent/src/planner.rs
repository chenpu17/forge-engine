//! Task planner and Todo management
//!
//! Manages task decomposition and tracking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Todo item for task tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// Unique ID
    pub id: String,
    /// Task description
    pub content: String,
    /// Current status
    pub status: TodoStatus,
    /// When the task was created
    pub created_at: DateTime<Utc>,
    /// When the task was completed
    pub completed_at: Option<DateTime<Utc>>,
}

impl TodoItem {
    /// Create a new pending todo item
    #[must_use]
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content: content.into(),
            status: TodoStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
        }
    }
}

/// Todo status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task is pending
    Pending,
    /// Task is in progress
    InProgress,
    /// Task is completed
    Completed,
}

/// Task planner manages todo items
#[derive(Debug, Default)]
pub struct Planner {
    /// List of todo items
    todos: Vec<TodoItem>,
    /// Index of currently active task
    current: Option<usize>,
}

impl Planner {
    /// Create a new planner
    #[must_use]
    pub const fn new() -> Self {
        Self { todos: Vec::new(), current: None }
    }

    /// Add a new task
    ///
    /// # Panics
    ///
    /// This function will not panic in practice, as it always pushes an item before accessing it.
    #[allow(clippy::expect_used)]
    pub fn add(&mut self, content: impl Into<String>) -> &TodoItem {
        let item = TodoItem::new(content);
        self.todos.push(item);
        self.todos.last().expect("just pushed")
    }

    /// Add multiple tasks at once
    pub fn add_many(&mut self, tasks: impl IntoIterator<Item = impl Into<String>>) {
        for task in tasks {
            self.add(task);
        }
    }

    /// Start a task (set to in progress)
    pub fn start(&mut self, id: &str) -> Option<&TodoItem> {
        if let Some(idx) = self.todos.iter().position(|t| t.id == id) {
            self.todos[idx].status = TodoStatus::InProgress;
            self.current = Some(idx);
            Some(&self.todos[idx])
        } else {
            None
        }
    }

    /// Complete a task
    pub fn complete(&mut self, id: &str) -> Option<&TodoItem> {
        if let Some(idx) = self.todos.iter().position(|t| t.id == id) {
            self.todos[idx].status = TodoStatus::Completed;
            self.todos[idx].completed_at = Some(Utc::now());

            if self.current == Some(idx) {
                self.current = None;
            }

            Some(&self.todos[idx])
        } else {
            None
        }
    }

    /// Complete the current task
    pub fn complete_current(&mut self) -> Option<&TodoItem> {
        if let Some(idx) = self.current {
            let id = self.todos[idx].id.clone();
            self.complete(&id)
        } else {
            None
        }
    }

    /// Get the next pending task
    #[must_use]
    pub fn next_pending(&self) -> Option<&TodoItem> {
        self.todos.iter().find(|t| t.status == TodoStatus::Pending)
    }

    /// Get the current task
    #[must_use]
    pub fn current(&self) -> Option<&TodoItem> {
        self.current.map(|idx| &self.todos[idx])
    }

    /// Get progress (completed, total)
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let completed = self.todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
        (completed, self.todos.len())
    }

    /// Check if all tasks are complete
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.todos.is_empty() && self.todos.iter().all(|t| t.status == TodoStatus::Completed)
    }

    /// Check if there are any tasks
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.todos.is_empty()
    }

    /// Get all todos
    #[must_use]
    pub fn todos(&self) -> &[TodoItem] {
        &self.todos
    }

    /// Get todos as owned vector (for events)
    #[must_use]
    pub fn todos_owned(&self) -> Vec<TodoItem> {
        self.todos.clone()
    }

    /// Clear all todos
    pub fn clear(&mut self) {
        self.todos.clear();
        self.current = None;
    }

    /// Remove a specific todo
    pub fn remove(&mut self, id: &str) -> Option<TodoItem> {
        if let Some(idx) = self.todos.iter().position(|t| t.id == id) {
            if self.current == Some(idx) {
                self.current = None;
            } else if let Some(current) = self.current {
                if current > idx {
                    self.current = Some(current - 1);
                }
            }
            Some(self.todos.remove(idx))
        } else {
            None
        }
    }

    /// Update todo list from a list (replaces existing)
    pub fn update(&mut self, todos: Vec<TodoItem>) {
        self.todos = todos;
        self.current = self.todos.iter().position(|t| t.status == TodoStatus::InProgress);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_todo() {
        let mut planner = Planner::new();
        let item = planner.add("Test task");

        assert_eq!(item.content, "Test task");
        assert_eq!(item.status, TodoStatus::Pending);
        assert_eq!(planner.todos.len(), 1);
    }

    #[test]
    fn test_start_and_complete() {
        let mut planner = Planner::new();
        let id = planner.add("Test task").id.clone();

        planner.start(&id);
        assert_eq!(planner.current().unwrap().status, TodoStatus::InProgress);

        planner.complete(&id);
        assert_eq!(planner.todos[0].status, TodoStatus::Completed);
        assert!(planner.current().is_none());
    }

    #[test]
    fn test_progress() {
        let mut planner = Planner::new();
        planner.add_many(["Task 1", "Task 2", "Task 3"]);

        assert_eq!(planner.progress(), (0, 3));

        let id = planner.todos[0].id.clone();
        planner.complete(&id);

        assert_eq!(planner.progress(), (1, 3));
    }

    #[test]
    fn test_next_pending() {
        let mut planner = Planner::new();
        planner.add_many(["Task 1", "Task 2"]);

        let next = planner.next_pending().unwrap();
        assert_eq!(next.content, "Task 1");

        let id = planner.todos[0].id.clone();
        planner.complete(&id);

        let next = planner.next_pending().unwrap();
        assert_eq!(next.content, "Task 2");
    }

    #[test]
    fn test_is_complete() {
        let mut planner = Planner::new();
        assert!(!planner.is_complete()); // Empty is not complete

        planner.add("Task 1");
        assert!(!planner.is_complete());

        let id = planner.todos[0].id.clone();
        planner.complete(&id);
        assert!(planner.is_complete());
    }

    #[test]
    fn test_remove() {
        let mut planner = Planner::new();
        let id = planner.add("Task 1").id.clone();
        planner.add("Task 2");

        let removed = planner.remove(&id);
        assert!(removed.is_some());
        assert_eq!(planner.todos.len(), 1);
        assert_eq!(planner.todos[0].content, "Task 2");
    }
}
