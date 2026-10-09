//! Single-instance file opening for the graphical interface: a second
//! `slate-gui FILE` hands its files to the running window and exits, like
//! desktop editors. Directories and `--new-instance` always start a window.
use crate::cli::LaunchFile;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Serialize, Deserialize)]
struct Request {
    files: Vec<(PathBuf, Option<usize>, Option<usize>)>,
    #[serde(default)]
    wait: bool,
}

/// Files handed over by other `slate-gui` invocations, waiting to be opened.
pub struct OpenRequest {
    pub files: Vec<LaunchFile>,
    pub(crate) ticket: Option<WaitTicket>,
}
impl From<LaunchFile> for OpenRequest {
    fn from(file: LaunchFile) -> Self {
        Self {
            files: vec![file],
            ticket: None,
        }
    }
}
pub type Inbox = Arc<Mutex<Vec<OpenRequest>>>;

/// Launch options that affect editing or layout belong to their own window.
/// Forwarding them could change existing documents or discard recovery state.
pub fn can_share_window(launch: &crate::cli::Launch) -> bool {
    !launch.new_instance
        && !launch.read_only
        && launch.recover
        && launch.startup.is_none()
        && launch.directory.is_none()
        && launch.stdin.is_none()
        && launch.options.is_empty()
        && !launch.ignore_rc
}

#[cfg(unix)]
fn socket_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let uid = unsafe { libc::geteuid() };
    base.join(format!("slate-gui-{uid}.sock"))
}

/// Send files to a running instance, returning immediately after acceptance.
pub fn forward(files: &[LaunchFile]) -> bool {
    forward_request(files, false).unwrap_or(false)
}
/// Wait for the requested files to close. Only an absent server permits fallback.
pub fn forward_wait(files: &[LaunchFile]) -> anyhow::Result<bool> {
    forward_request(files, true)
}
#[cfg(unix)]
fn forward_request(files: &[LaunchFile], wait: bool) -> anyhow::Result<bool> {
    use std::io::Write;
    let mut stream = match std::os::unix::net::UnixStream::connect(socket_path()) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false)
        }
        Err(error) => return Err(error.into()),
    };
    stream.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;
    let request = Request {
        files: files
            .iter()
            .map(|f| Ok((crate::document::absolute(&f.path)?, f.line, f.column)))
            .collect::<anyhow::Result<_>>()?,
        wait,
    };
    let mut json = serde_json::to_vec(&request)?;
    anyhow::ensure!(
        json.len() < 1024 * 1024 && files.len() <= 128,
        "Too many files in editor request"
    );
    json.push(b'\n');
    stream.write_all(&json)?;
    wait_reply(stream, wait)?;
    Ok(true)
}
#[cfg(not(unix))]
fn forward_request(_files: &[LaunchFile], wait: bool) -> anyhow::Result<bool> {
    anyhow::ensure!(!wait, "--wait is supported on Unix systems");
    Ok(false)
}
// A private inherited socket connects a waiting launcher to a newly created GUI.
const WAIT_FD: &str = "SLATE_INTERNAL_WAIT_FD";
static WAIT_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct WaitTicket {
    #[cfg(unix)]
    stream: std::os::unix::net::UnixStream,
}
impl WaitTicket {
    #[cfg(unix)]
    fn new(stream: std::os::unix::net::UnixStream) -> anyhow::Result<Self> {
        stream.set_write_timeout(Some(std::time::Duration::from_millis(10)))?;
        anyhow::ensure!(
            crate::budget::reserve(&WAIT_COUNT, 1, 128),
            "Too many waiting editor requests"
        );
        Ok(Self { stream })
    }
    /// Consume the one inherited descriptor once, before starting other threads.
    pub fn inherited() -> anyhow::Result<Option<Self>> {
        let Some(value) = std::env::var_os(WAIT_FD) else {
            return Ok(None);
        };
        std::env::remove_var(WAIT_FD);
        #[cfg(unix)]
        {
            use std::os::fd::FromRawFd;
            let fd: i32 = value.to_string_lossy().parse()?;
            anyhow::ensure!(fd >= 3, "Invalid editor wait descriptor");
            // Validate before assuming ownership of a descriptor supplied in the environment.
            let mut kind: libc::c_int = 0;
            let mut len = std::mem::size_of_val(&kind) as libc::socklen_t;
            anyhow::ensure!(
                unsafe {
                    libc::getsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_TYPE,
                        (&mut kind as *mut libc::c_int).cast(),
                        &mut len,
                    )
                } == 0
                    && kind == libc::SOCK_STREAM,
                "Invalid editor wait socket"
            );
            anyhow::ensure!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
                "Cannot protect editor wait descriptor"
            );
            Self::new(unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) }).map(Some)
        }
        #[cfg(not(unix))]
        {
            let _ = value;
            anyhow::bail!("--wait is supported on Unix systems")
        }
    }
    pub(crate) fn ready(&self) -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use std::io::Write;
            (&self.stream).write_all(b"ok\n")?;
        }
        Ok(())
    }
    pub(crate) fn connected(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut byte = 0u8;
            let result = unsafe {
                libc::recv(
                    self.stream.as_raw_fd(),
                    (&mut byte as *mut u8).cast(),
                    1,
                    libc::MSG_PEEK | libc::MSG_DONTWAIT,
                )
            };
            result < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
    pub fn finish(self, error: Option<&str>) {
        #[cfg(unix)]
        {
            use std::io::Write;
            let message = error
                .map(|e| {
                    format!(
                        "error:{}\n",
                        e.chars()
                            .filter(|c| !c.is_control())
                            .take(1024)
                            .collect::<String>()
                    )
                })
                .unwrap_or_else(|| "done\n".into());
            let _ = (&self.stream).write_all(message.as_bytes());
        }
        #[cfg(not(unix))]
        {
            let _ = error;
        }
    }
}
impl Drop for WaitTicket {
    fn drop(&mut self) {
        WAIT_COUNT.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(unix)]
fn wait_reply(stream: std::os::unix::net::UnixStream, wait: bool) -> anyhow::Result<()> {
    use std::io::{BufRead, BufReader, Read};
    let mut reader = BufReader::new(stream);
    let mut read_reply = |expected: &str| -> anyhow::Result<()> {
        let mut reply = String::new();
        (&mut reader).take(4096).read_line(&mut reply)?;
        anyhow::ensure!(
            reply.trim() == expected,
            "{}",
            reply
                .strip_prefix("error:")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("GUI disconnected before editing completed")
        );
        Ok(())
    };
    read_reply("ok")?;
    if wait {
        // A completion is unbounded in time, never an arbitrary editing deadline.
        // Keep the same reader: acceptance and completion may arrive together.
        reader.get_ref().set_read_timeout(None)?;
        let mut reply = String::new();
        (&mut reader).take(4096).read_line(&mut reply)?;
        anyhow::ensure!(
            reply.trim() == "done",
            "{}",
            reply
                .strip_prefix("error:")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("GUI disconnected before editing completed")
        );
    }
    Ok(())
}

/// Start a GUI child and wait only for this invocation's files, not its window.
pub fn start_waiting_gui(arguments: &[std::ffi::OsString]) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::{
            os::{fd::AsRawFd, unix::process::CommandExt},
            process::{Command, Stdio},
        };
        let (client, server) = std::os::unix::net::UnixStream::pair()?;
        let fd = server.as_raw_fd();
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(arguments)
            .env(WAIT_FD, fd.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        drop(server);
        client.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        let result = wait_reply(client, true);
        if result.is_err() {
            // Reap failed startups, but leave a healthy GUI available after a
            // protocol error or caller cancellation.
            let _ = child.try_wait();
        }
        result
    }
    #[cfg(not(unix))]
    {
        let _ = arguments;
        anyhow::bail!("--wait is supported on Unix systems")
    }
}

/// Accept files from later invocations. `notify` wakes the event loop.
#[cfg(unix)]
pub fn listen(notify: impl Fn() + Send + 'static) -> Option<Inbox> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    let path = socket_path();
    // A leftover socket from a crashed instance refuses connections.
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path).ok()?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let inbox: Inbox = Default::default();
    let queue = inbox.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(500)));
            let mut line = String::new();
            use std::io::Read;
            let mut reader = BufReader::new(&stream).take(1024 * 1024);
            if reader.read_line(&mut line).is_err() || !line.ends_with('\n') {
                continue;
            }
            let Ok(request) = serde_json::from_str::<Request>(&line) else {
                continue;
            };
            let mut queued = queue.lock().unwrap();
            if queued.len() >= 128 || request.files.is_empty() || request.files.len() > 128 {
                let _ = (&stream).write_all(b"error:Too many pending editor requests\n");
                continue;
            }
            let ticket = if request.wait {
                match stream
                    .try_clone()
                    .map_err(anyhow::Error::from)
                    .and_then(WaitTicket::new)
                {
                    Ok(ticket) => Some(ticket),
                    Err(_) => {
                        let _ = (&stream).write_all(b"error:Too many waiting editor requests\n");
                        continue;
                    }
                }
            } else {
                None
            };
            if (&stream).write_all(b"ok\n").is_err() {
                continue;
            }
            queued.push(OpenRequest {
                files: request
                    .files
                    .into_iter()
                    .map(|(path, line, column)| LaunchFile { path, line, column })
                    .collect(),
                ticket,
            });
            drop(queued);
            notify();
        }
    });
    Some(inbox)
}
#[cfg(not(unix))]
pub fn listen(_notify: impl Fn() + Send + 'static) -> Option<Inbox> {
    None
}

/// Remove the socket when the primary instance exits.
pub fn release() {
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(socket_path());
    }
}

use crate::App;
impl App {
    pub fn attach_inbox(&mut self, inbox: Inbox) {
        self.inbox = Some(inbox);
    }
    /// Open files handed over by other invocations and ask to be raised.
    pub(crate) fn drain_inbox(&mut self) {
        let requests = match &self.inbox {
            Some(inbox) => std::mem::take(&mut *inbox.lock().unwrap()),
            None => return,
        };
        if requests.is_empty() {
            return;
        }
        for request in requests {
            self.open_editor_request(request);
        }
        self.frontend_requests.push("raise".into());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn launch_options_are_never_lost_to_forwarding() {
        let launch =
            |args: &[&str]| crate::cli::parse(args.iter().map(std::ffi::OsString::from)).unwrap();
        assert!(can_share_window(&launch(&["file.txt"])));
        for flag in [
            "--view",
            "--fresh",
            "--workspace",
            "--editor-only",
            "--new-instance",
        ] {
            assert!(!can_share_window(&launch(&[flag, "file.txt"])));
        }
    }

    #[test]
    fn a_second_invocation_hands_files_to_the_first() {
        // Other tests change the environment too.
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let runtime = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", runtime.path());
        let woke = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = woke.clone();
        let inbox = listen(move || flag.store(true, std::sync::atomic::Ordering::SeqCst)).unwrap();
        // A client that never sends a newline must not block later launches.
        let stalled = std::os::unix::net::UnixStream::connect(socket_path()).unwrap();
        let file = LaunchFile {
            path: runtime.path().join("a.txt"),
            line: Some(3),
            column: None,
        };
        assert!(forward(std::slice::from_ref(&file)));
        drop(stalled);
        let received = inbox.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].files[0].line, Some(3));
        assert!(woke.load(std::sync::atomic::Ordering::SeqCst));
        release();
        assert!(!forward(&[file]), "No instance after release");
    }
    fn fixture() -> (
        tempfile::TempDir,
        std::sync::MutexGuard<'static, ()>,
        crate::App,
    ) {
        let guard = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let path = dir.path().join("unrelated.txt");
        std::fs::write(&path, "unrelated").unwrap();
        let app = crate::App::new(&path).unwrap();
        (dir, guard, app)
    }
    fn client() -> (std::os::unix::net::UnixStream, WaitTicket) {
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(30)))
            .unwrap();
        (client, WaitTicket::new(server).unwrap())
    }
    fn blocked(client: &mut std::os::unix::net::UnixStream) {
        use std::io::Read;
        let error = client.read(&mut [0]).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }
    fn reply(client: &mut std::os::unix::net::UnixStream) -> String {
        use std::io::{BufRead, BufReader};
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        line
    }
    #[test]
    fn wait_is_a_file_scoped_launch_option() {
        let parse = |args: &[&str]| crate::cli::parse(args.iter().map(std::ffi::OsString::from));
        let launch = parse(&["--wait", "+2", "file.txt"]).unwrap();
        assert!(launch.wait && can_share_window(&launch));
        assert_eq!(launch.files[0].line, Some(2));
        assert!(parse(&["--wait"]).is_err());
        assert!(parse(&["--wait", "-"]).is_err());
        let mut oversized = vec!["--wait"];
        oversized.extend(std::iter::repeat_n("file.txt", 129));
        assert!(parse(&oversized).is_err());
        assert!(parse(&["--wait", "--help"]).is_ok());
        assert!(!can_share_window(
            &parse(&["--wait", "-I", "file.txt"]).unwrap()
        ));
        assert!(!can_share_window(
            &parse(&["--wait", "-B", "file.txt"]).unwrap()
        ));
    }
    #[test]
    fn wait_survives_save_as_cancel_and_split_until_last_view_closes() {
        let (dir, _guard, mut app) = fixture();
        let file = LaunchFile {
            path: dir.path().join("unrelated.txt"),
            ..Default::default()
        };
        let (mut client, ticket) = client();
        app.attach_editor_wait(ticket, &[file]).unwrap();
        assert_eq!(reply(&mut client), "ok\n");
        app.dispatch(crate::Command::Paste {
            text: "edited ".into(),
        });
        app.dispatch(crate::Command::CloseDocument { force: false });
        assert!(app.prompt.is_some());
        app.dispatch(crate::Command::DismissPrompt);
        blocked(&mut client);
        let target = dir.path().join("renamed.txt");
        app.dispatch(crate::Command::SaveAs {
            path: target.clone(),
            overwrite: false,
        });
        app.flush_workspace().unwrap();
        assert_eq!(std::fs::read_to_string(target).unwrap(), "edited unrelated");
        blocked(&mut client);
        app.dispatch(crate::Command::Split {
            axis: crate::layout::Axis::Horizontal,
            kind: None,
        });
        app.dispatch(crate::Command::CloseDocument { force: false });
        blocked(&mut client);
        let doc = *app
            .documents
            .iter()
            .find(|(_, d)| d.text() == "edited unrelated")
            .unwrap()
            .0;
        app.reveal_document(doc);
        app.dispatch(crate::Command::CloseDocument { force: false });
        assert_eq!(reply(&mut client), "done\n");
        assert!(!app.quit);
    }
    #[test]
    fn forwarded_waits_track_multiple_files_and_report_open_failure() {
        let (dir, _guard, mut app) = fixture();
        let files: Vec<_> = ["one.txt", "two.txt"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, name).unwrap();
                LaunchFile {
                    path,
                    ..Default::default()
                }
            })
            .collect();
        let (mut client, ticket) = client();
        ticket.ready().unwrap();
        assert_eq!(reply(&mut client), "ok\n");
        app.open_editor_request(OpenRequest {
            files: files.clone(),
            ticket: Some(ticket),
        });
        blocked(&mut client);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app.pending_open.is_empty() {
            app.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for (index, file) in files.iter().enumerate() {
            app.reveal_document(app.document_for(&file.path).unwrap());
            app.dispatch(crate::Command::CloseDocument { force: false });
            if index == 0 {
                blocked(&mut client);
            }
        }
        assert_eq!(reply(&mut client), "done\n");
        assert!(!app.quit);
        let (mut failed, ticket) = self::client();
        app.open_editor_request(OpenRequest {
            files: vec![LaunchFile {
                path: dir.path().join("missing-parent/file.txt"),
                ..Default::default()
            }],
            ticket: Some(ticket),
        });
        while !app.pending_open.is_empty() {
            app.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
            assert!(std::time::Instant::now() < deadline);
        }
        assert!(reply(&mut failed).starts_with("error:"));
    }
    #[test]
    fn wait_protocol_rejects_disconnects_and_accepts_coalesced_completion() {
        use std::io::Write;
        let (client, mut server) = std::os::unix::net::UnixStream::pair().unwrap();
        server.write_all(b"ok\ndone\n").unwrap();
        wait_reply(client, true).unwrap();
        let (client, mut server) = std::os::unix::net::UnixStream::pair().unwrap();
        server.write_all(b"ok\n").unwrap();
        drop(server);
        assert!(wait_reply(client, true).is_err());
    }
    #[test]
    fn abandoned_waiters_are_removed_and_termination_is_an_error() {
        let (dir, _guard, mut app) = fixture();
        let files = [LaunchFile {
            path: dir.path().join("unrelated.txt"),
            ..Default::default()
        }];
        let (client, ticket) = client();
        app.attach_editor_wait(ticket, &files).unwrap();
        drop(client);
        app.poll_editor_waits();
        assert!(app.editor_waits.is_empty());
        let (mut client, ticket) = self::client();
        app.attach_editor_wait(ticket, &files).unwrap();
        assert_eq!(reply(&mut client), "ok\n");
        app.finish_editor_waits(Some("GUI terminated"));
        assert_eq!(reply(&mut client), "error:GUI terminated\n");
    }
    #[test]
    fn waiting_client_capacity_is_bounded_and_released_on_drop() {
        let _guard = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut connections = Vec::new();
        for _ in 0..128 {
            connections.push(client());
        }
        let (_, server) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(WaitTicket::new(server).is_err());
        drop(connections);
        let (_, server) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(WaitTicket::new(server).is_ok());
    }
}
