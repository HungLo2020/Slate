use anyhow::{bail, Result};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Search {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
}
impl Search {
    pub fn regex(&self) -> Result<Regex> {
        if self.query.is_empty() {
            bail!("Enter text to find");
        }
        let query = regex::escape(&self.query);
        Ok(RegexBuilder::new(&if self.whole_word {
            format!(r"\b{query}\b")
        } else {
            query
        })
        .case_insensitive(!self.case_sensitive)
        .build()?)
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
