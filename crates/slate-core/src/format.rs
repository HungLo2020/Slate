//! Document formatting: the language server's formatter when it has one,
//! otherwise the language's command-line formatter (text on standard input,
//! formatted text on standard output). Format-on-save runs before writing.
use crate::{services::Reply, App};
use anyhow::{bail, Result};
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
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
    let mut child = Command::new(program)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("No standard input")?;
    let input = input.to_string();
    std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take().ok_or("No standard output")?;
    let mut stderr = child.stderr.take().ok_or("No standard error")?;
    let out = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stdout.read_to_end(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stderr.read_to_end(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} timed out"));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if !status.success() {
        let message = String::from_utf8_lossy(&stderr);
        return Err(format!(
            "{program}: {}",
            message
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("failed")
        ));
    }
    String::from_utf8(stdout).map_err(|_| format!("{program} wrote invalid UTF-8"))
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
        if !self.trusted {
            if then_save {
                return Ok(false);
            }
            bail!(
                "Formatters run code from this folder. Trust the workspace first (trust-workspace)"
            );
        }
        if self.documents[&doc].read_only {
            bail!("The document is read-only");
        }
        if self.lsp_format(doc, then_save) {
            self.formatting.insert(doc);
            self.status = "Formatting…".into();
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
            Reply::Formatted(
                doc,
                content,
                then_save,
                pipe(&command, &cwd, &text, TIMEOUT),
            )
        });
        self.formatting.insert(doc);
        self.status = "Formatting…".into();
        Ok(true)
    }

    pub(crate) fn formatted(
        &mut self,
        doc: u64,
        content: u64,
        then_save: bool,
        result: Result<String, String>,
    ) {
        let outcome = (|| -> Result<()> {
            let text = result.map_err(anyhow::Error::msg)?;
            let d = self
                .documents
                .get(&doc)
                .ok_or_else(|| anyhow::anyhow!("The document was closed"))?;
            if d.content_version() != content {
                bail!("The document changed while formatting; format again");
            }
            match minimal_edit(&d.text(), &text) {
                Some((start, end, value)) => {
                    let cursor = self
                        .views
                        .values()
                        .find(|v| v.document == doc)
                        .map_or(0, |v| v.cursor);
                    self.documents
                        .get_mut(&doc)
                        .unwrap()
                        .replace(start, end, &value, cursor)?;
                    self.rebase_views(doc, start, end, value.len());
                    self.clamp_views(doc);
                    self.status = "Formatted".into();
                }
                None => self.status = "Already formatted".into(),
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            self.status = format!("Format failed: {e:#}");
        }
        if then_save {
            if let Err(e) = self.finish_format_on_save(doc) {
                self.status = format!("Error: {e:#}");
            }
        } else {
            self.formatting.remove(&doc);
        }
    }

    /// Save after format-on-save, whatever the formatter did.
    pub(crate) fn finish_format_on_save(&mut self, doc: u64) -> Result<()> {
        self.formatting.remove(&doc);
        if self.documents.contains_key(&doc) {
            self.start_save(doc, None, false, false)?;
        }
        Ok(())
    }
}

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
