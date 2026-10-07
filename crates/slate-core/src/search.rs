use anyhow::{bail, Result};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
pub type RopeRegex = regex_cursor::engines::meta::Regex;
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Search {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    #[serde(skip)]
    compiled: std::sync::OnceLock<std::result::Result<Regex, String>>,
    #[serde(skip)]
    rope: std::sync::OnceLock<std::result::Result<std::sync::Arc<RopeRegex>, String>>,
}
impl Search {
    pub fn new(query: String, case_sensitive: bool, whole_word: bool) -> Self {
        Self {
            query,
            case_sensitive,
            whole_word,
            compiled: Default::default(),
            rope: Default::default(),
        }
    }
    /// The literal query as a pattern, with case and word options as flags.
    fn pattern(&self) -> Result<String> {
        if self.query.is_empty() {
            bail!("Enter text to find");
        }
        let query = regex::escape(&self.query);
        let query = if self.whole_word {
            format!(r"\b{query}\b")
        } else {
            query
        };
        Ok(if self.case_sensitive {
            query
        } else {
            format!("(?i){query}")
        })
    }
    /// For searching strings (a line or a visible window).
    pub fn regex(&self) -> Result<&Regex> {
        let pattern = self.pattern()?;
        let compiled = self.compiled.get_or_init(|| {
            RegexBuilder::new(&pattern)
                .build()
                .map_err(|e| e.to_string())
        });
        compiled.as_ref().map_err(|e| anyhow::anyhow!(e.clone()))
    }
    /// For searching whole documents without copying them.
    pub fn rope_regex(&self) -> Result<std::sync::Arc<RopeRegex>> {
        let pattern = self.pattern()?;
        let compiled = self.rope.get_or_init(|| {
            RopeRegex::new(&pattern)
                .map(std::sync::Arc::new)
                .map_err(|e| e.to_string())
        });
        compiled.clone().map_err(|e| anyhow::anyhow!(e))
    }
    /// A key identifying the compiled pattern, for caches.
    pub fn key(&self) -> String {
        format!(
            "{}\u{0}{}{}",
            self.query, self.case_sensitive, self.whole_word
        )
    }
}
/// All matches of a pattern in a rope, as byte ranges.
pub fn find_all(regex: &RopeRegex, rope: &ropey::Rope) -> Vec<(usize, usize)> {
    regex
        .find_iter(regex_cursor::Input::new(rope))
        .map(|m| (m.start(), m.end()))
        .collect()
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
