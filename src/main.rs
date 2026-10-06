use anyhow::{bail, Result};
use std::path::PathBuf;
fn main() -> Result<()> {
    let mut args = std::env::args_os();
    let invocation = args.next().unwrap_or_default();
    let mut gui = PathBuf::from(invocation)
        .file_name()
        .map(|n| n == "slate-gui")
        .unwrap_or(false);
    let mut path = None;
    let mut recover = true;
    for arg in args {
        match arg.to_str() {
            Some("--fresh") => recover = false,
            Some("--gui") => gui = true,
            Some("--tui") => gui = false,
            Some("--help") | Some("-h") => {
                println!("Slate — shared Rust editor\n\nslate [--tui|--gui] [--fresh] [FILE|DIRECTORY]\nslate-gui [FILE|DIRECTORY]\n\nF1: commands  F6: next pane  F7: next tab  F8: new terminal\nF9: split right  Shift-F9: split below\nEditor: Ctrl-S save, Ctrl-Z undo, Ctrl-Y redo, Ctrl-F find, Ctrl-H replace, Ctrl-G line\n--fresh: start without restoring this workspace\nCtrl-Shift-P: commands (also works from terminal panes)");
                return Ok(());
            }
            Some("--version") => {
                println!("Slate {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some(s) if s.starts_with('-') => bail!("Unknown argument: {s}"),
            _ => {
                if path.replace(PathBuf::from(arg)).is_some() {
                    bail!("Pass one file or workspace directory");
                }
            }
        }
    }
    let path = path.unwrap_or(std::env::current_dir()?);
    let mut app = slate_core::App::new(&path)?;
    if let Err(e) = app.enable_workspace(recover) {
        app.status = format!("Workspace recovery disabled: {e:#}");
    }
    if gui {
        #[cfg(feature = "gui")]
        {
            let code = slate_gui::run(app);
            if code != 0 {
                bail!("GUI exited with code {code}");
            }
        }
        #[cfg(not(feature = "gui"))]
        {
            let _ = app;
            bail!("This build excludes the GUI. Rebuild with default features");
        }
    } else {
        slate_cli::run(app)?;
    }
    Ok(())
}
