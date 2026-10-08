//! Running helper programs (Git, formatters, tools) with a time limit. Each
//! runs in its own process group so that a timeout, or quitting, stops
//! everything it started, not just the first process.
use std::{
    io::{Read, Write},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Send `signal` to the process group led by `pid`.
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

/// Run `command` with `input` on standard input, collecting its output.
/// After `timeout` the whole process group is killed and an error names
/// `label`.
pub fn run(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    run_inner(command, input, timeout, label, false)
}

/// Keep password prompts visible while still bounding the helper's lifetime.
pub fn run_interactive(
    command: &mut Command,
    input: Vec<u8>,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    run_inner(command, Some(input), timeout, label, true)
}

fn run_inner(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    label: &str,
    interactive: bool,
) -> Result<Output, String> {
    const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
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
    let mut child = command.spawn().map_err(|e| format!("{label}: {e}"))?;
    let (done, completed) = mpsc::channel();
    let mut input_done = input.is_none();
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let done = done.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
            drop(stdin);
            let _ = done.send((2, Ok(Vec::new())));
        });
    }
    fn drain(
        pipe: Option<impl Read + Send + 'static>,
        which: usize,
        done: mpsc::Sender<(usize, Result<Vec<u8>, String>)>,
    ) {
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
    drain(child.stderr.take(), 1, done);
    let mut output = [None, None];
    let started = Instant::now();
    let mut status = None;
    loop {
        let result = (|| {
            while let Ok((which, bytes)) = completed.try_recv() {
                let bytes = bytes.map_err(|e| format!("{label}: {e}"))?;
                if which == 2 {
                    input_done = true;
                } else {
                    output[which] = Some(bytes);
                }
            }
            if status.is_none() {
                status = child.try_wait().map_err(|e| format!("{label}: {e}"))?;
            }
            if status.is_some() && input_done && output.iter().all(Option::is_some) {
                return Ok(true);
            }
            if started.elapsed() >= timeout {
                return Err(format!(
                    "{label} timed out after {} s",
                    timeout.as_secs().max(1)
                ));
            }
            Ok(false)
        })();
        match result {
            Ok(true) => break,
            Ok(false) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                stop_group(&mut child, Duration::ZERO);
                return Err(error);
            }
        }
    }
    Ok(Output {
        status: status.unwrap(),
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
}
