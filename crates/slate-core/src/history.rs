//! Search and replacement history, optionally persisted.
use crate::search::Prompt;
use crate::{
    user_state::{path as state, read},
    App,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct History {
    #[serde(default)]
    values: BTreeMap<String, Vec<String>>,
    #[serde(skip)]
    positions: BTreeMap<String, usize>,
    #[serde(skip)]
    drafts: BTreeMap<String, String>,
    #[serde(skip)]
    loaded: bool,
}
impl History {
    pub(crate) fn begin(&mut self) {
        self.positions.clear();
        self.drafts.clear();
    }
    fn load(&mut self, persist: bool) {
        if !self.loaded {
            if persist {
                self.values = read::<History>("history.json").values;
                self.values.retain(|_, v| {
                    v.retain(|s| s.len() <= 4096);
                    v.truncate(100);
                    true
                });
            }
            self.loaded = true;
        }
    }
    fn remember(&mut self, key: &str, value: &str) {
        if value.is_empty() || value.len() > 4096 {
            return;
        }
        let list = self.values.entry(key.into()).or_default();
        list.retain(|s| s != value);
        list.insert(0, value.into());
        list.truncate(100);
        self.positions.remove(key);
        self.drafts.remove(key);
    }
}
impl App {
    pub(crate) fn remember_prompt_history(&mut self, p: &Prompt) {
        if !matches!(p.kind.as_str(), "find" | "replace") {
            return;
        }
        self.history.load(self.preferences.history_log);
        self.history.remember("find", &p.input);
        if p.kind == "replace" {
            self.history.remember("replace", &p.replacement);
        }
        if self.preferences.history_log {
            if let Ok(json) = serde_json::to_vec(&self.history) {
                let _ = crate::fsio::write_private(&state("history.json"), &json);
            }
        }
    }
    pub(crate) fn search_history(&mut self, backward: bool) -> Result<()> {
        self.history.load(self.preferences.history_log);
        let p = self
            .prompt
            .as_mut()
            .filter(|p| matches!(p.kind.as_str(), "find" | "replace"))
            .context("Open find or replace first")?;
        let (key, value) = if p.field == 1 {
            ("replace", &mut p.replacement)
        } else {
            ("find", &mut p.input)
        };
        let Some(list) = self.history.values.get(key).filter(|v| !v.is_empty()) else {
            return Ok(());
        };
        self.history
            .drafts
            .entry(key.into())
            .or_insert_with(|| value.clone());
        let pos = self.history.positions.entry(key.into()).or_insert(0);
        *pos = if backward {
            (*pos + 1).min(list.len())
        } else {
            pos.saturating_sub(1)
        };
        *value = if *pos == 0 {
            self.history.drafts[key].clone()
        } else {
            list[*pos - 1].clone()
        };
        Ok(())
    }
}
