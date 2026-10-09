//! `slate-gui`: the graphical interface. `--tui` runs the terminal interface
//! from the same executable.
use anyhow::{bail, Result};
use slate_core::cli::{self, Frontend};
use std::ffi::OsString;

/// Qt's own command-line options and whether each takes a value.
const QT_OPTIONS: &[(&str, bool)] = &[
    ("-platform", true),
    ("-platformpluginpath", true),
    ("-platformtheme", true),
    ("-plugin", true),
    ("-qwindowgeometry", true),
    ("-qwindowicon", true),
    ("-qwindowtitle", true),
    ("-display", true),
    ("-style", true),
    ("-stylesheet", true),
    ("-session", true),
    ("-name", true),
    ("-reverse", false),
    ("-widgetcount", false),
    ("-nograb", false),
    ("-dograb", false),
    ("-sync", false),
];

/// Separate Qt options (passed to QApplication) from Slate's arguments.
fn split_arguments(args: Vec<OsString>) -> (Vec<OsString>, Vec<String>) {
    let mut slate = vec![];
    let mut qt = vec![];
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let text = arg.to_string_lossy().into_owned();
        let name = text.split('=').next().unwrap_or_default().to_string();
        match QT_OPTIONS
            .iter()
            .find(|(option, _)| *option == name || format!("-{option}") == name)
        {
            Some((_, takes_value)) => {
                qt.push(text.clone());
                if *takes_value && !text.contains('=') {
                    if let Some(value) = iter.next() {
                        qt.push(value.to_string_lossy().into_owned());
                    }
                }
            }
            None if text.starts_with("-qmljsdebugger") => qt.push(text),
            None => slate.push(arg),
        }
    }
    (slate, qt)
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(result) = slate_core::fsio::elevated_save_entry(&arguments) {
        return result;
    }
    let original_arguments = arguments.clone();
    let (args, qt_arguments) = split_arguments(arguments);
    let launch = cli::parse(args)?;
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
    let inherited_wait = slate_core::instance::WaitTicket::inherited()?;
    let smoke = std::env::var_os("SLATE_GUI_SMOKE_DIR").is_some();
    let instance_enabled = !smoke || std::env::var_os("SLATE_GUI_WAIT_SMOKE").is_some();
    let single = slate_core::instance::can_share_window(&launch) && instance_enabled;
    if inherited_wait.is_none() {
        if launch.wait {
            if single && slate_core::instance::forward_wait(&launch.files)? {
                return Ok(());
            }
            return slate_core::instance::start_waiting_gui(&original_arguments);
        }
        if single && !launch.files.is_empty() && slate_core::instance::forward(&launch.files) {
            return Ok(());
        }
    }
    // Waiting launchers keep normal signal handling; only the GUI masks signals.
    slate_gui::handle_termination_signals();
    if smoke {
        eprintln!("Smoke startup: constructing shared core");
    }
    let mut app = match cli::start(&launch) {
        Ok(app) => app,
        Err(error) => {
            if let Some(ticket) = inherited_wait {
                ticket.finish(Some(&error.to_string()));
            }
            return Err(error);
        }
    };
    if let Some(ticket) = inherited_wait {
        app.attach_editor_wait(ticket, &launch.files)?;
    }
    if smoke {
        eprintln!("Smoke startup: core constructed");
        eprintln!("Smoke startup: workspace ready");
    }
    let owns_socket = if instance_enabled && !launch.read_only && launch.stdin.is_none() {
        let events = app.events();
        match slate_core::instance::listen(move || events.notify()) {
            Some(inbox) => {
                app.attach_inbox(inbox);
                true
            }
            None => false,
        }
    } else {
        false
    };
    let code = slate_gui::run_with(app, &qt_arguments);
    if owns_socket {
        slate_core::instance::release();
    }
    if code != 0 {
        bail!("GUI exited with code {code}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::split_arguments;

    #[test]
    fn qt_options_are_separated_from_files() {
        let args = [
            "-platform",
            "offscreen",
            "file.txt",
            "-reverse",
            "--style=Fusion",
            "+3",
            "b",
        ]
        .map(std::ffi::OsString::from)
        .to_vec();
        let (slate, qt) = split_arguments(args);
        assert_eq!(slate, ["file.txt", "+3", "b"].map(std::ffi::OsString::from));
        assert_eq!(qt, ["-platform", "offscreen", "-reverse", "--style=Fusion"]);
    }
}
