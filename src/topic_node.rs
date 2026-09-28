use std::collections::HashMap;
use chrono::{DateTime, Local};

#[derive(Debug, Default)]
pub struct TopicNode {
    pub name: String,
    pub children: HashMap<String, TopicNode>,
    pub is_leaf: bool,
    pub messages: Vec<(DateTime<Local>, String)>,
}

impl TopicNode {
    pub fn insert(&mut self, path: &[&str]) -> &mut TopicNode {
        if path.is_empty() {
            self.is_leaf = true;
            return self;
        }

        let part = path[0];
        let child = self.children.entry(part.to_string()).or_insert_with(|| TopicNode {
            name: part.to_string(),
            ..Default::default()
        });

        child.insert(&path[1..])
    }

    pub fn add_message(&mut self, path: &[&str], content: String, max_messages: usize) {
        let node = self.insert(path);
        let now = Local::now();
        node.messages.push((now, content));

        if node.messages.len() > max_messages {
            node.messages.drain(0..node.messages.len() - max_messages);
        }
    }

    pub fn clear_all_messages(&mut self) {
        self.messages.clear();
        for child in self.children.values_mut() {
            child.clear_all_messages();
        }
    }

    /// Get full topic path for this node
    pub fn get_full_path(&self, parent_path: &str) -> String {
        if parent_path.is_empty() || parent_path == "root" {
            self.name.clone()
        } else {
            format!("{}/{}", parent_path, self.name)
        }
    }

    /// Recursively collect all leaf topics
    pub fn collect_leaf_topics(&self, parent_path: &str, topics: &mut Vec<String>) {
        let current_path = self.get_full_path(parent_path);

        if self.children.is_empty() && !self.messages.is_empty() {
            topics.push(current_path);
        } else {
            for child in self.children.values() {
                child.collect_leaf_topics(&current_path, topics);
            }
        }
    }

    /// Find a node by path
    #[allow(dead_code)]
    pub fn find_node(&self, path: &[&str]) -> Option<&TopicNode> {
        if path.is_empty() {
            return Some(self);
        }
        self.children.get(path[0])?.find_node(&path[1..])
    }

    /// Try to parse messages as numeric values for plotting
    #[allow(dead_code)]
    pub fn get_numeric_values(&self) -> Vec<[f64; 2]> {
        let mut values = Vec::new();

        if let Some(first_time) = self.messages.first().map(|(t, _)| t.timestamp_millis()) {
            for (timestamp, msg) in &self.messages {
                // Try to parse the message as a number
                if let Some(value) = parse_numeric(msg) {
                    let time_offset = (timestamp.timestamp_millis() - first_time) as f64 / 1000.0;
                    values.push([time_offset, value]);
                }
            }
        }

        values
    }
}

/// Parse a string as f64, also accepting "true"/"false" (case-insensitive).
pub fn parse_numeric(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    match trimmed.to_lowercase().as_str() {
        "true" => Some(1.0),
        "false" => Some(0.0),
        _ => trimmed.parse::<f64>().ok(),
    }
}
