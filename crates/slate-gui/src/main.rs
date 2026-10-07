//! `slate-gui`: the graphical interface. `--tui` runs the terminal interface
//! from the same executable.
use anyhow::{bail, Result};
use slate_core::cli::{self, Frontend};

fn main() -> Result<()> {
    let launch = cli::parse(std::env::args_os().skip(1))?;
    if launch.help {
        println!("{}", cli::USAGE);
        return Ok(());
    }
    if launch.version {
        println!("Slate {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if launch.frontend == Some(Frontend::Tui) {
        return slate_cli::run(cli::start(&launch)?);
    }
    let smoke = std::env::var_os("SLATE_GUI_SMOKE_DIR").is_some();
    if smoke {
        eprintln!("Smoke startup: constructing shared core");
    }
    let app = cli::start(&launch)?;
    if smoke {
        eprintln!("Smoke startup: core constructed");
        eprintln!("Smoke startup: workspace ready");
    }
    let code = slate_gui::run(app);
    if code != 0 {
        bail!("GUI exited with code {code}");
    }
    Ok(())
}
