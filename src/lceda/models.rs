use serde_json::Value;

use crate::util::{nested_string, sanitize_filename};

#[derive(Debug, Clone)]
pub struct SearchItem {
    pub index: usize,
    pub display_title: String,
    pub title: String,
    pub manufacturer: String,
    pub model_uuid: Option<String>,
    pub raw: Value,
}

impl SearchItem {
    pub fn display_name(&self) -> &str {
        if !self.display_title.is_empty() {
            &self.display_title
        } else if !self.title.is_empty() {
            &self.title
        } else {
            "component"
        }
    }

    pub fn choose_step_filename(&self) -> String {
        let base = nested_string(&self.raw, &["footprint", "display_title"])
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| self.display_name().to_string());
        format!("{}.step", sanitize_filename(&base))
    }

    pub fn choose_obj_basename(&self) -> String {
        sanitize_filename(if !self.title.is_empty() {
            &self.title
        } else {
            self.display_name()
        })
    }

    pub fn symbol_uuid(&self) -> Option<String> {
        nested_string(&self.raw, &["symbol", "uuid"])
            .or_else(|| nested_string(&self.raw, &["attributes", "Symbol"]))
    }

    pub fn footprint_uuid(&self) -> Option<String> {
        nested_string(&self.raw, &["footprint", "uuid"])
            .or_else(|| nested_string(&self.raw, &["attributes", "Footprint"]))
    }
}
