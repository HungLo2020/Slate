//! Running helper programs (Git, formatters, tools) with a time limit. Each
//! runs in its own process group so that a timeout, or quitting, stops
//! everything it started, not just the first process.
use std::{
    io::{Read, Write},
    process::{Child, Command, Output, Stdio},
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
    isolate(command)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("{label}: {e}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                #[cfg(unix)]
                kill_group(child.id(), libc::SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{label} timed out after {} s",
                    timeout.as_secs().max(1)
                ));
            }
            Err(e) => return Err(format!("{label}: {e}")),
        }
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
