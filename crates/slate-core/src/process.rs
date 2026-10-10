//! Running helper programs (Git, formatters, tools) with a time limit. Each
//! runs in its own process group so that a timeout, or quitting, stops
//! everything it started, not just the first process.
//!
//! Helpers never inherit what describes another Git repository, and never
//! search relative `PATH` entries: the C library's `execvp` and
//! `posix_spawnp` (which `Command` uses on Linux) treat an empty entry or
//! `.` as the current folder, which may be an untrusted checkout. See
//! [`sanitize`].
use std::{
    ffi::OsString,
    io::{Read, Write},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Variables through which Git describes the repository it is working in.
/// Git exports some of them to hooks and to `core.editor`; a Slate started
/// as the commit editor must not hand them on, or `git -C <root>`, language
/// servers and shells in its terminals would use that repository and index
/// instead of their own. This is Git's own `local_repo_env` list, which it
/// clears when it runs Git in another repository.
pub const REPOSITORY_ENV: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

/// Remove [`REPOSITORY_ENV`] from what `command` inherits.
pub fn scrub_repository_env(command: &mut Command) -> &mut Command {
    for name in REPOSITORY_ENV {
        command.env_remove(name);
    }
    command
}

/// The search path for helpers when nothing usable is inherited. An empty
/// or missing `PATH` is not safe to pass on: the C library would search
/// the current folder (an empty `PATH` is one empty entry), or use a
/// built-in default, which in older versions began with it.
pub const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// `path` without its relative entries (empty ones mean the current
/// folder), `DEFAULT_PATH` when no entry is left, or `None` when it needs
/// no change.
pub fn absolute_path_entries(path: &std::ffi::OsStr) -> Option<OsString> {
    let entries: Vec<_> = std::env::split_paths(path).collect();
    if entries.iter().all(|entry| entry.is_absolute()) {
        return None;
    }
    let absolute: Vec<_> = entries.into_iter().filter(|e| e.is_absolute()).collect();
    if absolute.is_empty() {
        return Some(DEFAULT_PATH.into());
    }
    std::env::join_paths(absolute).ok()
}

/// Prepare a helper: no parent repository's environment, and a `PATH`
/// (the one set on `command`, else Slate's) without relative entries, so
/// a bare program name never runs a file from the current folder. Not used
/// for the shells of terminals, whose environment is the user's own.
pub fn sanitize(command: &mut Command) -> &mut Command {
    scrub_repository_env(command);
    let path = match command.get_envs().find(|(name, _)| *name == "PATH") {
        Some((_, value)) => value.map(OsString::from),
        None => std::env::var_os("PATH"),
    };
    let safe = match path {
        Some(path) => absolute_path_entries(&path),
        None => Some(DEFAULT_PATH.into()),
    };
    if let Some(safe) = safe {
        command.env("PATH", safe);
    }
    command
}

/// Send `signal` to the process group led by `pid`. The caller must not
/// have reaped `pid` yet, or the group's ID may belong to someone else.
pub fn kill_group(pid: u32, signal: i32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
    #[cfg(not(unix))]
    let _ = (pid, signal);
}

/// Ask a process group to stop, then force it after `grace`.
pub fn stop_group(child: &mut Child, grace: Duration) {
    #[cfg(unix)]
    kill_group(child.id(), libc::SIGTERM);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    #[cfg(unix)]
    kill_group(child.id(), libc::SIGKILL);
    let _ = child.kill();
    let _ = child.wait();
}

/// Put `command` in its own process group.
pub fn isolate(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

/// Block until the process `pid` (a child) has exited, without reaping it:
/// its process group cannot be reused while it is unreaped, so the group
/// can still be signalled safely.
#[cfg(unix)]
pub fn wait_exited(pid: u32) {
    wait_child(pid, false);
}

/// Like `wait_exited`; with `stops`, also return when the child is stopped
/// (Ctrl+Z), and say whether it was.
#[cfg(unix)]
fn wait_child(pid: u32, stops: bool) -> bool {
    let flags = libc::WEXITED | libc::WNOWAIT | if stops { libc::WSTOPPED } else { 0 };
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) };
        if result == 0 {
            return info.si_code == libc::CLD_STOPPED;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return false;
        }
    }
}

/// Keep Slate running while an interactive helper has the terminal: Ctrl+C,
/// Ctrl+\ and Ctrl+Z reach a handler that does nothing, until dropped,
/// which restores the previous dispositions. A handler rather than
/// `SIG_IGN`, because an ignored signal stays ignored across `exec`: the
/// helper (and anything else started meanwhile) gets the default action
/// and stays cancellable.
///
/// Dispositions are process-wide, so guards share one count: the first
/// installs the handler and the last restores, whichever threads hold them
/// and in whatever order they are dropped.
#[cfg(unix)]
pub struct HoldTerminalSignals {
    _held: (),
}
/// Live guards, and the dispositions from before the first.
#[cfg(unix)]
static HELD: std::sync::Mutex<(usize, Vec<(libc::c_int, libc::sigaction)>)> =
    std::sync::Mutex::new((0, Vec::new()));
#[cfg(unix)]
impl HoldTerminalSignals {
    pub fn new() -> Self {
        extern "C" fn ignore(_: libc::c_int) {}
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        if held.0 == 0 {
            held.1.clear();
            for signal in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP] {
                unsafe {
                    let mut action: libc::sigaction = std::mem::zeroed();
                    action.sa_sigaction = ignore as *const () as libc::sighandler_t;
                    action.sa_flags = libc::SA_RESTART;
                    libc::sigemptyset(&mut action.sa_mask);
                    let mut old: libc::sigaction = std::mem::zeroed();
                    if libc::sigaction(signal, &action, &mut old) == 0 {
                        held.1.push((signal, old));
                    }
                }
            }
        }
        held.0 += 1;
        Self { _held: () }
    }
}
#[cfg(unix)]
impl Default for HoldTerminalSignals {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(unix)]
impl Drop for HoldTerminalSignals {
    fn drop(&mut self) {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        held.0 -= 1;
        if held.0 == 0 {
            for (signal, old) in held.1.drain(..) {
                unsafe {
                    libc::sigaction(signal, &old, std::ptr::null_mut());
                }
            }
        }
    }
}

/// Make process group `group` the foreground of terminal `tty`. Blocking
/// SIGTTOU lets a background process group (the helper before `exec`,
/// Slate afterwards) do this without being stopped. Async-signal-safe.
#[cfg(unix)]
fn set_foreground(tty: libc::c_int, group: libc::pid_t) {
    unsafe {
        let mut block: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGTTOU);
        libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut old);
        libc::tcsetpgrp(tty, group);
        libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
    }
}

/// The controlling terminal, while an interactive helper's process group is
/// its foreground: keys (the password, Ctrl+C) go to the helper, and the
/// helper may read the terminal without being stopped by SIGTTIN. Dropping
/// it gives the terminal back to Slate's process group.
#[cfg(unix)]
struct Foreground {
    tty: std::fs::File,
    slate: libc::pid_t,
}
#[cfg(unix)]
impl Foreground {
    /// The controlling terminal, if Slate's process group has it.
    fn claim() -> Option<Self> {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open("/dev/tty")
            .ok()?;
        let slate = unsafe { libc::getpgrp() };
        (unsafe { libc::tcgetpgrp(tty.as_raw_fd()) } == slate).then_some(Self { tty, slate })
    }
    /// Have `command` take the terminal for its own process group before it
    /// starts, so it never runs in the background.
    fn hand_to(&self, command: &mut Command) {
        use std::os::{fd::AsRawFd, unix::process::CommandExt};
        let tty = self.tty.as_raw_fd();
        unsafe {
            command.pre_exec(move || {
                set_foreground(tty, libc::getpid());
                Ok(())
            });
        }
    }
}
#[cfg(unix)]
impl Drop for Foreground {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        set_foreground(self.tty.as_raw_fd(), self.slate);
    }
}

/// Run `command` with `input` on standard input, collecting its output.
/// After `timeout` the whole process group is killed and an error names
/// `label`. The helper is [`sanitize`]d first.
pub fn run(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    run_inner(command, input, timeout, label, false)
}

/// Keep password prompts visible while still bounding the helper's lifetime.
/// The helper's process group becomes the terminal's foreground, so its
/// prompt can read the keyboard and Ctrl+C cancels the helper, not Slate;
/// Slate takes the terminal back however the helper ends.
pub fn run_interactive(
    command: &mut Command,
    input: Vec<u8>,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    run_inner(command, Some(input), timeout, label, true)
}

/// What the helper threads report: standard output and error (0, 1), the
/// input written (2), the helper exited (3), an interactive helper was
/// stopped (4).
type Done = (usize, Result<Vec<u8>, String>);

fn run_inner(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    label: &str,
    interactive: bool,
) -> Result<Output, String> {
    const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
    sanitize(command);
    isolate(command)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(if interactive {
            Stdio::inherit()
        } else {
            Stdio::piped()
        });
    // Declared before the child, so dropped after it is reaped: the
    // terminal returns to Slate, then Ctrl+C may stop Slate again.
    #[cfg(unix)]
    let _signals = interactive.then(HoldTerminalSignals::new);
    #[cfg(unix)]
    let _foreground = interactive
        .then(Foreground::claim)
        .flatten()
        .inspect(|f| f.hand_to(command));
    let mut child = command.spawn().map_err(|e| format!("{label}: {e}"))?;
    let (done, completed) = mpsc::channel::<Done>();
    let mut input_done = input.is_none();
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let done = done.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
            drop(stdin);
            let _ = done.send((2, Ok(Vec::new())));
        });
    }
    fn drain(pipe: Option<impl Read + Send + 'static>, which: usize, done: mpsc::Sender<Done>) {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let result = pipe
                    .by_ref()
                    .take((OUTPUT_LIMIT + 1) as u64)
                    .read_to_end(&mut bytes);
                if let Err(error) = result {
                    let _ = done.send((which, Err(error.to_string())));
                    return;
                }
                if bytes.len() > OUTPUT_LIMIT {
                    let _ = done.send((which, Err("output exceeded 16 MiB".into())));
                    return;
                }
            }
            let _ = done.send((which, Ok(bytes)));
        });
    }
    drain(child.stdout.take(), 0, done.clone());
    drain(child.stderr.take(), 1, done.clone());
    // The helper is reaped here, after any timeout kill of its group.
    #[cfg(unix)]
    {
        let (done, pid) = (done.clone(), child.id());
        std::thread::spawn(move || {
            let stopped = wait_child(pid, interactive);
            let _ = done.send((if stopped { 4 } else { 3 }, Ok(Vec::new())));
        });
    }
    let mut output = [None, None];
    let deadline = Instant::now() + timeout;
    let mut exited = false;
    let result = loop {
        if exited && input_done && output.iter().all(Option::is_some) {
            break Ok(());
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        // Without `waitid`, look at the helper now and then.
        #[cfg(not(unix))]
        let wait = wait.min(Duration::from_millis(20));
        match completed.recv_timeout(wait) {
            Ok((3, _)) => exited = true,
            Ok((2, _)) => input_done = true,
            // Ctrl+Z at a password prompt: the helper has the terminal, so
            // nobody would resume it. Cancel rather than hang.
            Ok((4, _)) => {
                #[cfg(unix)]
                {
                    kill_group(child.id(), libc::SIGKILL);
                    kill_group(child.id(), libc::SIGCONT);
                }
                break Err(format!("{label} was cancelled"));
            }
            Ok((which, Ok(bytes))) => output[which] = Some(bytes),
            Ok((_, Err(error))) => break Err(format!("{label}: {error}")),
            Err(_) => {
                #[cfg(not(unix))]
                if matches!(child.try_wait(), Ok(Some(_))) {
                    exited = true;
                    continue;
                }
                if Instant::now() >= deadline {
                    break Err(format!(
                        "{label} timed out after {} s",
                        timeout.as_secs().max(1)
                    ));
                }
            }
        }
    };
    if let Err(error) = result {
        stop_group(&mut child, Duration::ZERO);
        return Err(error);
    }
    Ok(Output {
        status: child.wait().map_err(|e| format!("{label}: {e}"))?,
        stdout: output[0].take().unwrap(),
        stderr: output[1].take().unwrap(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_covers_pipes_inherited_by_descendants() {
        let started = Instant::now();
        let result = run(
            Command::new("sh").args(["-c", "sleep 5 & exit 0"]),
            None,
            Duration::from_millis(100),
            "orphan",
        );
        assert!(result.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn excessive_output_is_bounded() {
        let result = run(
            Command::new("head").args(["-c", "20000000", "/dev/zero"]),
            None,
            Duration::from_secs(5),
            "noisy",
        );
        assert!(result.unwrap_err().contains("output exceeded"));
    }

    #[test]
    fn timeouts_stop_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        // A pipeline: killing only `sh` would leave `sleep` running.
        let script = format!("(sleep 1; touch '{}') & wait", marker.display());
        let started = Instant::now();
        let result = run(
            Command::new("sh").args(["-c", &script]),
            None,
            Duration::from_millis(200),
            "script",
        );
        assert_eq!(result.unwrap_err(), "script timed out after 1 s");
        assert!(started.elapsed() < Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(1300));
        assert!(!marker.exists(), "a child outlived the timeout");
        let output = run(
            Command::new("tr").args(["a-z", "A-Z"]),
            Some(b"abc".to_vec()),
            Duration::from_secs(5),
            "tr",
        )
        .unwrap();
        assert_eq!(output.stdout, b"ABC");
    }

    #[test]
    fn helpers_get_neither_a_parent_repository_nor_relative_path_entries() {
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "echo \"${GIT_DIR-unset} ${GIT_INDEX_FILE-unset} $PATH\"",
            ])
            .env("GIT_DIR", "/elsewhere/.git")
            .env("GIT_INDEX_FILE", "/elsewhere/.git/index")
            .env("PATH", "/usr/bin::.:bin:/bin:");
        let output = run(&mut command, None, Duration::from_secs(5), "sh").unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "unset unset /usr/bin:/bin\n"
        );
        assert_eq!(absolute_path_entries("/usr/bin:/bin".as_ref()), None);
    }

    /// Tests that change process-wide signal dispositions.
    #[cfg(unix)]
    static SIGNALS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    fn disposition(signal: libc::c_int) -> libc::sighandler_t {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            libc::sigaction(signal, std::ptr::null(), &mut action);
            action.sa_sigaction
        }
    }

    #[cfg(unix)]
    #[test]
    fn signal_guards_restore_when_the_last_one_ends_in_any_order() {
        let _serial = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
        let before = disposition(libc::SIGINT);
        let first = HoldTerminalSignals::new();
        let held = disposition(libc::SIGINT);
        assert_ne!(held, before);
        let second = std::thread::spawn(HoldTerminalSignals::new).join().unwrap();
        // Released out of order, on another thread.
        drop(first);
        assert_eq!(disposition(libc::SIGINT), held);
        std::thread::spawn(move || drop(second)).join().unwrap();
        assert_eq!(disposition(libc::SIGINT), before);
    }

    #[cfg(unix)]
    #[test]
    fn a_stopped_interactive_helper_is_cancelled() {
        let _serial = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
        // Ctrl+Z at a password prompt stops the helper, which has the
        // terminal: nobody would resume it.
        let started = Instant::now();
        let result = run_interactive(
            Command::new("sh").args(["-c", "kill -TSTP $$; echo resumed"]),
            Vec::new(),
            Duration::from_secs(60),
            "sudo",
        );
        // Not the timeout: the stop itself ends the wait.
        assert_eq!(result.unwrap_err(), "sudo was cancelled");
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn an_empty_or_relative_only_path_becomes_a_safe_default() {
        for path in ["", ":", ".:bin"] {
            assert_eq!(
                absolute_path_entries(path.as_ref()),
                Some(DEFAULT_PATH.into()),
                "{path:?}"
            );
            let mut command = Command::new("sh");
            command.env("PATH", path);
            sanitize(&mut command);
            let set = command.get_envs().find(|(name, _)| *name == "PATH");
            assert_eq!(set, Some(("PATH".as_ref(), Some(DEFAULT_PATH.as_ref()))));
        }
    }

    #[cfg(unix)]
    #[test]
    fn slate_survives_terminal_signals_while_an_interactive_helper_runs() {
        let _serial = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
        let before = disposition(libc::SIGINT);
        // What Ctrl+C, Ctrl+\ and Ctrl+Z would send to Slate before and
        // after the helper has the terminal. Without a handler Slate (this
        // test) would die or stop here.
        let output = run_interactive(
            Command::new("sh").args([
                "-c",
                "kill -INT $PPID; kill -QUIT $PPID; kill -TSTP $PPID; cat",
            ]),
            b"still here".to_vec(),
            Duration::from_secs(5),
            "helper",
        )
        .unwrap();
        assert_eq!(output.stdout, b"still here");
        assert_eq!(disposition(libc::SIGINT), before, "dispositions restored");
        // The helper itself still gets the default action.
        let output = run_interactive(
            Command::new("sh").args(["-c", "kill -INT $$; echo survived"]),
            Vec::new(),
            Duration::from_secs(5),
            "helper",
        )
        .unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(output.status.signal(), Some(libc::SIGINT));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn helpers_are_waited_for_until_they_exit() {
        // Without polling: the exit is noticed promptly, also when the
        // helper closed its output long before.
        let started = Instant::now();
        let output = run(
            Command::new("sh").args(["-c", "sleep 0.3; echo done"]),
            None,
            Duration::from_secs(30),
            "sleeper",
        )
        .unwrap();
        assert_eq!(output.stdout, b"done\n");
        assert!(started.elapsed() < Duration::from_secs(10));
        let output = run(
            Command::new("sh").args(["-c", "exec >&- 2>&-; sleep 0.2; exit 3"]),
            None,
            Duration::from_secs(5),
            "closed",
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(3));
    }
}
