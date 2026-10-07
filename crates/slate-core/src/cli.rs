//! Command-line parsing shared by the `slate` and `slate-gui` executables.
use crate::preferences::StartupMode;
use anyhow::{bail, Result};
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frontend {
    Gui,
    Tui,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchFile {
    pub path: PathBuf,
    /// One-based line and column from `+LINE[,COLUMN]`.
    pub line: Option<usize>,
    pub column: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub frontend: Option<Frontend>,
    pub files: Vec<LaunchFile>,
    pub directory: Option<PathBuf>,
    /// `-` reads the initial buffer from standard input.
    pub stdin: Option<LaunchFile>,
    pub read_only: bool,
    pub recover: bool,
    pub startup: Option<StartupMode>,
    /// GUI: open a separate window instead of handing files to a running one.
    pub new_instance: bool,
    pub help: bool,
    pub version: bool,
}

pub const USAGE: &str = "\
Slate — shared Rust editor

Usage:
  slate [OPTIONS] [+LINE[,COLUMN]] [FILE...]
  slate [OPTIONS] DIRECTORY
  command | slate -            edit standard input
  slate-gui [OPTIONS] [FILE...|DIRECTORY]

Options:
  --tui / --gui      choose the terminal or graphical interface
  -v, --view         open files read-only
  --fresh            start without restoring the previous workspace
  --editor-only      show only the editor, regardless of startup settings
  --workspace        show the full workspace, regardless of startup settings
  --new-instance     (GUI) open a new window instead of reusing a running Slate
  -h, --help         show this help
  -V, --version      show the version

Files that do not exist are created when first saved. +LINE[,COLUMN] or
+LINE:COLUMN positions the cursor in the file that follows it.

Keys: F1 / Ctrl-Shift-P commands · F6 next pane · F7 next tab · F8 terminal
F9 split right · Shift-F9 split below · F10 expand/collapse workspace
Editor: Ctrl-S save, Ctrl-Z undo, Ctrl-Y redo, Ctrl-F find, Ctrl-H replace,
Ctrl-G go to line, Ctrl-Q quit. A nano-compatible keymap is available in
Settings (keymap = \"nano\").";

fn position(spec: &str) -> Result<(usize, Option<usize>)> {
    let (line, column) = match spec.split_once([',', ':']) {
        Some((line, column)) => (line, Some(column)),
        None => (spec, None),
    };
    let line = if line.is_empty() {
        1
    } else {
        line.parse()
            .map_err(|_| anyhow::anyhow!("Invalid line number: +{spec}"))?
    };
    let column = column
        .filter(|c| !c.is_empty())
        .map(|c| c.parse())
        .transpose()
        .map_err(|_| anyhow::anyhow!("Invalid column: +{spec}"))?;
    Ok((line, column))
}

/// Parse arguments after the program name.
pub fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Result<Launch> {
    let mut launch = Launch {
        recover: true,
        ..Default::default()
    };
    let mut pending: Option<(usize, Option<usize>)> = None;
    let mut options = true;
    for arg in args {
        let text = arg.to_str();
        if options {
            match text {
                Some("--") => {
                    options = false;
                    continue;
                }
                Some("--fresh") => {
                    launch.recover = false;
                    continue;
                }
                Some("--editor-only") => {
                    launch.startup = Some(StartupMode::EditorOnly);
                    continue;
                }
                Some("--workspace") => {
                    launch.startup = Some(StartupMode::Workspace);
                    continue;
                }
                Some("--gui") => {
                    launch.frontend = Some(Frontend::Gui);
                    continue;
                }
                Some("--tui") => {
                    launch.frontend = Some(Frontend::Tui);
                    continue;
                }
                Some("-v" | "--view") => {
                    launch.read_only = true;
                    continue;
                }
                Some("--new-instance") => {
                    launch.new_instance = true;
                    continue;
                }
                Some("-h" | "--help") => {
                    launch.help = true;
                    continue;
                }
                Some("-V" | "--version") => {
                    launch.version = true;
                    continue;
                }
                Some("-") => {
                    if launch.stdin.is_some() {
                        bail!("Standard input can only be read once");
                    }
                    let (line, column) = pending.take().unzip();
                    launch.stdin = Some(LaunchFile {
                        path: PathBuf::new(),
                        line,
                        column: column.flatten(),
                    });
                    continue;
                }
                Some(s) if s.starts_with('+') && s.len() > 1 => {
                    pending = Some(position(&s[1..])?);
                    continue;
                }
                Some(s) if s.starts_with('-') => bail!("Unknown argument: {s}\nRun slate --help"),
                _ => {}
            }
        }
        let path = PathBuf::from(arg);
        if path.is_dir() {
            if launch.directory.is_some() || !launch.files.is_empty() {
                bail!("Pass one workspace directory, or files to edit");
            }
            launch.directory = Some(path);
            pending = None;
        } else {
            if launch.directory.is_some() {
                bail!("Pass one workspace directory, or files to edit");
            }
            let (line, column) = pending.take().unzip();
            launch.files.push(LaunchFile {
                path,
                line,
                column: column.flatten(),
            });
        }
    }
    if pending.is_some() {
        bail!("+LINE must be followed by a file");
    }
    Ok(launch)
}

/// Create the session for a launch: read standard input when requested and
/// enable recovery for directory workspaces (and files, when configured).
pub fn start(launch: &Launch) -> Result<crate::App> {
    let stdin = if launch.stdin.is_some() {
        use std::io::Read;
        let mut bytes = vec![];
        std::io::stdin().read_to_end(&mut bytes)?;
        Some(bytes)
    } else {
        None
    };
    let mut app = crate::App::launch(launch, stdin)?;
    if app.wants_recovery() {
        if let Err(e) = app.enable_workspace(launch.recover) {
            app.status = format!("Workspace recovery disabled: {e:#}");
        }
    }
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(list: &[&str]) -> Result<Launch> {
        parse(list.iter().map(OsString::from))
    }

    #[test]
    fn positions_files_and_flags() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("missing.txt");
        let launch = args(&["+12,4", file.to_str().unwrap(), "-v", "other.rs"]).unwrap();
        assert!(launch.read_only);
        assert_eq!(launch.files.len(), 2);
        assert_eq!(launch.files[0].line, Some(12));
        assert_eq!(launch.files[0].column, Some(4));
        assert_eq!(launch.files[1].line, None);
        let launch = args(&["+7:2", "-"]).unwrap();
        assert_eq!(launch.stdin.unwrap().line, Some(7));
        let launch = args(&[dir.path().to_str().unwrap(), "--gui", "--fresh"]).unwrap();
        assert_eq!(launch.directory.as_deref(), Some(dir.path()));
        assert_eq!(launch.frontend, Some(Frontend::Gui));
        assert!(!launch.recover);
        assert!(args(&["+x", "a"]).is_err());
        assert!(args(&["+3"]).is_err());
        assert!(args(&["--bogus"]).is_err());
        assert!(args(&[dir.path().to_str().unwrap(), "file"]).is_err());
        let literal = args(&["--", "-v", "+3"]).unwrap();
        assert_eq!(literal.files.len(), 2);
        assert!(!literal.read_only);
    }
}
