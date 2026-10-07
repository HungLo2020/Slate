//! `slate`: the terminal interface. It does not link Qt, so it runs on
//! servers and minimal systems; `--gui` hands over to `slate-gui`.
use anyhow::{bail, Result};
use slate_core::cli::{self, Frontend};
use std::{ffi::OsString, path::PathBuf};

fn graphical_executable() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?.canonicalize().ok()?;
    // An older installation made `slate-gui` a symlink to `slate`; never
    // exec ourselves.
    let distinct = |p: &PathBuf| p.is_file() && p.canonicalize().ok().as_ref() != Some(&current);
    let sibling = current.with_file_name("slate-gui");
    if distinct(&sibling) {
        return Some(sibling);
    }
    slate_core::fsio::which("slate-gui").filter(distinct)
}

fn run_gui(args: Vec<OsString>) -> Result<()> {
    let Some(program) = graphical_executable() else {
        bail!(
            "The graphical interface (slate-gui) is not installed. Install the Qt \
             components (the slate package's recommended dependencies) or run slate \
             in a terminal without --gui"
        );
    };
    let mut command = std::process::Command::new(&program);
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        bail!("Cannot start {}: {error}", program.display());
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args_os();
    let invocation = PathBuf::from(args.next().unwrap_or_default());
    let args: Vec<OsString> = args.collect();
    let launch = cli::parse(args.clone())?;
    let invoked_as_gui = invocation.file_name().is_some_and(|n| n == "slate-gui");
    if launch.frontend == Some(Frontend::Gui)
        || (invoked_as_gui && launch.frontend != Some(Frontend::Tui))
    {
        return run_gui(args);
    }
    if launch.help {
        println!("{}", cli::USAGE);
        return Ok(());
    }
    if launch.version {
        println!("Slate {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    slate_cli::run(cli::start(&launch)?)
}
