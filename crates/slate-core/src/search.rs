use anyhow::{bail, Result};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Search {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    #[serde(skip)]
    compiled: std::sync::OnceLock<std::result::Result<Regex, String>>,
}
impl Search {
    pub fn new(query: String, case_sensitive: bool, whole_word: bool) -> Self {
        Self {
            query,
            case_sensitive,
            whole_word,
            compiled: Default::default(),
        }
    }
    pub fn regex(&self) -> Result<&Regex> {
        if self.query.is_empty() {
            bail!("Enter text to find");
        }
        let compiled = self.compiled.get_or_init(|| {
            let query = regex::escape(&self.query);
            RegexBuilder::new(&if self.whole_word {
                format!(r"\b{query}\b")
            } else {
                query
            })
            .case_insensitive(!self.case_sensitive)
            .build()
            .map_err(|e| e.to_string())
        });
        compiled.as_ref().map_err(|e| anyhow::anyhow!(e.clone()))
    }
}
#[derive(Clone, Serialize)]
pub struct Prompt {
    pub kind: String,
    pub input: String,
    pub replacement: String,
    pub field: usize,
    pub case_sensitive: bool,
    pub whole_word: bool,
}
