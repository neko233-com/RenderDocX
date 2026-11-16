//! Named event-filter expressions, shared by the desktop and Agent CLI.
use crate::buffer_formats::FormatStore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFilter {
    pub name: String,
    pub expression: String,
}

pub struct FilterStore(FormatStore);
impl FilterStore {
    fn validate_name(name: &str) -> Result<(), String> {
        if name.is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(
                "Filter name must be nonempty, trimmed and contain no control characters".into(),
            );
        }
        Ok(())
    }
    pub fn new() -> Result<Self, String> {
        let root = FormatStore::new()?;
        let parent = root
            .directory()
            .parent()
            .ok_or("No configuration directory available")?;
        Ok(Self::at(parent.join("event-filters")))
    }
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        Self(FormatStore::at(directory))
    }
    pub fn save(&self, name: &str, expression: &str) -> Result<SavedFilter, String> {
        Self::validate_name(name)?;
        // Encoding allows the valid empty expression without changing format-store semantics.
        let stored = self.0.save(
            name,
            &serde_json::to_string(expression).map_err(|e| e.to_string())?,
        )?;
        Ok(SavedFilter {
            name: stored.name,
            expression: expression.to_owned(),
        })
    }
    pub fn get(&self, name: &str) -> Result<SavedFilter, String> {
        Self::validate_name(name)?;
        let stored = self.0.get(name)?;
        let expression = serde_json::from_str(&stored.layout)
            .map_err(|e| format!("Invalid saved filter: {e}"))?;
        Ok(SavedFilter {
            name: stored.name,
            expression,
        })
    }
    pub fn delete(&self, name: &str) -> Result<(), String> {
        Self::validate_name(name)?;
        self.0.delete(name)
    }
    pub fn list(&self) -> Result<Vec<SavedFilter>, String> {
        self.0
            .list()?
            .into_iter()
            .map(|stored| {
                let expression = serde_json::from_str(&stored.layout)
                    .map_err(|e| format!("Invalid saved filter: {e}"))?;
                Ok(SavedFilter {
                    name: stored.name,
                    expression,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips_empty_unicode_and_future_expressions() {
        let dir = tempfile::tempdir().unwrap();
        let store = FilterStore::at(dir.path());
        store.save("../测试", "").unwrap();
        store.save("Future", "$param(name: value)").unwrap();
        assert_eq!(store.get("../测试").unwrap().expression, "");
        assert_eq!(store.list().unwrap().len(), 2);
        store.save("../测试", "$action()").unwrap();
        assert_eq!(store.get("../测试").unwrap().expression, "$action()");
        store.delete("../测试").unwrap();
        assert!(store.get("../测试").is_err());
    }
}
