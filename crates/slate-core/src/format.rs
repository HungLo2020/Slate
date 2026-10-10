//! Document formatting: the language server's formatter when it has one,
//! otherwise the language's command-line formatter (text on standard input,
//! formatted text on standard output). Format-on-save runs before writing.
use crate::{services::Reply, App};
use anyhow::{bail, Result};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(30);

/// Run `command` with `input` on standard input, killing it after `timeout`.
pub fn pipe(
    command: &[String],
    cwd: &Path,
    input: &str,
    timeout: Duration,
) -> Result<String, String> {
    pipe_env(command, cwd, input, &[], timeout)
}

/// `pipe` with extra environment variables.
pub fn pipe_env(
    command: &[String],
    cwd: &Path,
    input: &str,
    env: &[(String, String)],
    timeout: Duration,
) -> Result<String, String> {
    let (program, args) = command.split_first().ok_or("Empty command")?;
    let output = crate::process::run(
        Command::new(program)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd),
        Some(input.as_bytes().to_vec()),
        timeout,
        program,
    )?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{program}: {}",
            message
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{program} wrote invalid UTF-8"))
}

/// The smallest single replacement turning `old` into `new`.
pub fn minimal_edit(old: &str, new: &str) -> Option<(usize, usize, String)> {
    if old == new {
        return None;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some((
        prefix,
        old.len() - suffix,
        new[prefix..new.len() - suffix].to_string(),
    ))
}

impl App {
    /// Start formatting a document. Returns false when no formatter applies
    /// (only possible with `then_save`, which then saves unformatted).
    pub(crate) fn format_document(&mut self, doc: u64, then_save: bool) -> Result<bool> {
        // Formatters read (and some execute) project configuration found
        // from the file's folder upwards.
        let folder = self.documents[&doc]
            .path
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.root.clone());
        if !self.trusted_path(&folder) {
            if then_save {
                return Ok(false);
            }
            let root = self
                .document_root(doc)
                .filter(|root| !self.trusted_path(root))
                .unwrap_or(folder);
            self.ask_trust_for(
                &root,
                "Formatters read and can run the project's configuration",
                crate::Command::Action {
                    name: "format-document".into(),
                    argument: String::new(),
                },
            );
            return Ok(false);
        }
        if self.documents[&doc].read_only {
            bail!("The document is read-only");
        }
        if self.lsp_format(doc) {
            self.begin_format(doc, then_save);
            return Ok(true);
        }
        let formatter = self.language_for(doc).and_then(|l| l.formatter);
        let installed = formatter
            .as_ref()
            .and_then(|c| c.first())
            .is_some_and(|p| crate::fsio::which(p).is_some() || Path::new(p).is_file());
        let (Some(command), true) = (formatter, installed) else {
            if then_save {
                return Ok(false);
            }
            bail!(
                "No formatter for this document. Install one or set `formatter` in languages.toml"
            );
        };
        let d = &self.documents[&doc];
        let path = d.path.clone().unwrap_or_default();
        let command = crate::languages::expand(&command, &path);
        let cwd = path.parent().map_or(self.root.clone(), Path::to_path_buf);
        let text = d.text();
        let content = d.content_version();
        self.services.background(move || {
            Reply::Formatted(doc, content, pipe(&command, &cwd, &text, TIMEOUT))
        });
        self.begin_format(doc, then_save);
        Ok(true)
    }

    pub(crate) fn begin_format(&mut self, doc: u64, then_save: bool) {
        self.formatting.insert(
            doc,
            Formatting {
                started: Instant::now(),
                then_save,
            },
        );
        self.status = "Formatting…".into();
    }

    /// Finish a format request however it ended: release the document and,
    /// for format-on-save, save it (formatted or not).
    pub(crate) fn end_format(&mut self, doc: u64, outcome: Result<&str>) {
        let Some(request) = self.formatting.remove(&doc) else {
            return;
        };
        self.status = match outcome {
            Ok(message) => message.to_string(),
            Err(e) => format!("Format failed: {e:#}"),
        };
        if request.then_save && self.documents.contains_key(&doc) {
            if let Err(e) = self.start_save(doc, None, false, false) {
                self.status = format!("Error: {e:#}");
            }
        }
    }

    /// Give up on formatters that did not answer.
    pub(crate) fn expire_formatting(&mut self) {
        let expired: Vec<u64> = self
            .formatting
            .iter()
            .filter(|(_, f)| f.started.elapsed() > REQUEST_TIMEOUT)
            .map(|(doc, _)| *doc)
            .collect();
        for doc in expired {
            self.lsp_cancel_formatting(doc);
            self.end_format(doc, Err(anyhow::anyhow!("the formatter did not answer")));
        }
    }

    pub(crate) fn formatted(&mut self, doc: u64, content: u64, result: Result<String, String>) {
        if !self.formatting.contains_key(&doc) {
            return;
        }
        let outcome = (|| -> Result<&str> {
            let text = result.map_err(anyhow::Error::msg)?;
            // Buffers hold `\n` lines; saving restores the file's own style.
            let text = crate::text_format::normalize_input(&text);
            let d = self
                .documents
                .get(&doc)
                .ok_or_else(|| anyhow::anyhow!("the document was closed"))?;
            if d.content_version() != content {
                bail!("the document changed while formatting; format again");
            }
            let current = d.text();
            // An empty answer for real content is a broken formatter, not a
            // request to delete the document.
            if text.trim().is_empty() && !current.trim().is_empty() {
                bail!("the formatter printed nothing");
            }
            match minimal_edit(&current, &text) {
                Some(edit) => {
                    self.replace_in_document(doc, &[edit])?;
                    Ok("Formatted")
                }
                None => Ok("Already formatted"),
            }
        })();
        self.end_format(doc, outcome);
    }
}

/// A format request in flight.
pub(crate) struct Formatting {
    started: Instant,
    then_save: bool,
}

/// How long a formatter (or language server) may take to format.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(35);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_edits_keep_common_text() {
        assert_eq!(minimal_edit("abc", "abc"), None);
        assert_eq!(
            minimal_edit("let x=1;\n", "let x = 1;\n"),
            Some((5, 6, " = ".into()))
        );
        assert_eq!(minimal_edit("é\n", "è\n"), Some((0, 2, "è".into())));
        assert_eq!(minimal_edit("aa", "aaa"), Some((2, 2, "a".into())));
    }

    #[test]
    fn piped_formatters_report_output_errors_and_timeouts() {
        let dir = std::env::temp_dir();
        let tr = vec!["tr".to_string(), "a-z".into(), "A-Z".into()];
        assert_eq!(pipe(&tr, &dir, "abc\n", TIMEOUT).unwrap(), "ABC\n");
        let fail = vec![
            "sh".to_string(),
            "-c".into(),
            "echo bad input >&2; exit 3".into(),
        ];
        assert_eq!(pipe(&fail, &dir, "", TIMEOUT).unwrap_err(), "sh: bad input");
        let slow = vec!["sleep".to_string(), "5".into()];
        assert!(pipe(&slow, &dir, "", Duration::from_millis(200))
            .unwrap_err()
            .contains("timed out"));
    }
}
