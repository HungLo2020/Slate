mod common;
use slate_core::{
    cli::{self, Frontend, Launch},
    document::Document,
    media::{MediaControl, MAX_MEDIA_BYTES},
    preferences::StartupMode,
    App, Command,
};
use std::{fs, path::Path, path::PathBuf};

const REPO_MEDIA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/media");

fn launch(path: &Path) -> Launch {
    Launch {
        frontend: Some(Frontend::Gui),
        directory: Some(path.into()),
        startup: Some(StartupMode::EditorOnly),
        recover: true,
        ..Default::default()
    }
}
fn fixture(name: &str) -> PathBuf {
    Path::new(REPO_MEDIA).join(name)
}
/// A copy of a fixture in the test's own folder, so nothing writes the repo.
fn copy(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::copy(fixture(name), &path).unwrap();
    path
}
fn active(app: &App) -> u64 {
    match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => *id,
        _ => panic!("no active document"),
    }
}
fn open(app: &mut App, path: &Path) -> u64 {
    app.dispatch(Command::Open { path: path.into() });
    common::settle(app, "audio opening", |a| {
        a.documents
            .values()
            .any(|d| d.path.as_deref() == Some(path) && d.media.is_some())
    });
    app.views[&active(app)].document
}
fn control(app: &mut App, control: MediaControl) {
    app.dispatch(Command::MediaControl {
        pane: app.focus,
        control,
    });
}

#[test]
fn gui_opens_audio_tabs_and_text_mode_refuses_them() {
    let (dir, _guard) = common::isolated();
    let path = copy(dir.path(), "tone.wav");
    let mut app = App::launch(&launch(dir.path()), None).unwrap();
    let id = open(&mut app, &path);
    let document = &app.documents[&id];
    assert!(document.media.is_some());
    assert!(document.read_only);
    assert!(document.is_empty(), "an audio tab holds no text");
    assert!(!document.dirty());
    assert!(document.media.as_ref().unwrap().bytes <= MAX_MEDIA_BYTES);
    // Editing and saving are refused, like an image preview.
    app.dispatch(Command::Key {
        key: slate_core::Key {
            key: "x".into(),
            text: "x".into(),
            ..Default::default()
        },
    });
    assert!(app.documents[&id].is_empty());
    app.dispatch(Command::Save);
    common::settle(&mut app, "refused audio save", |a| {
        a.status.contains("Previews cannot be saved")
    });
    // The tab is presented as audio, with no text screen behind it.
    let frame = app.snapshot(100, 40, 0, 1, 1, 1);
    let pane = frame.panes.iter().find(|p| p.focused).unwrap();
    assert_eq!(pane.kind, "media");
    assert!(pane.media.is_some());
    assert!(pane.screen.is_none());
    // Without a graphical frontend the file is still a binary file.
    assert!(Document::open_for_frontend(&path, false).is_err());
}

#[test]
fn extensions_and_headers_identify_audio_without_one() {
    let (dir, _guard) = common::isolated();
    for name in [
        "tone.wav",
        "tone.mp3",
        "tone.ogg",
        "tone.flac",
        "tone.m4a",
        "tone.opus",
    ] {
        let path = fixture(name);
        assert!(
            slate_core::media::media_extension(&path),
            "{name} has a known extension"
        );
        assert!(
            Document::open_for_frontend(&path, true)
                .unwrap()
                .media
                .is_some(),
            "{name} opens in an audio tab"
        );
    }
    // A video container is not an audio tab, even though it shares `ftyp`.
    let video = dir.path().join("clip.mp4");
    let mut mp4 = b"\0\0\0\x18ftypisom".to_vec();
    mp4.extend_from_slice(&[0; 32]);
    fs::write(&video, &mp4).unwrap();
    assert!(Document::open_for_frontend(&video, true).is_err());

    let extensionless = copy(dir.path(), "tone.flac");
    let renamed = dir.path().join("voice");
    fs::rename(&extensionless, &renamed).unwrap();
    assert!(Document::open_for_frontend(&renamed, true)
        .unwrap()
        .media
        .is_some());
}

#[test]
fn transport_commands_validate_and_track_recovery() {
    let (dir, _guard) = common::isolated();
    let path = copy(dir.path(), "tone.wav");
    let mut app = App::launch(&launch(dir.path()), None).unwrap();
    let _ = open(&mut app, &path);
    let view = active(&app);
    assert_eq!(app.views[&view].media.volume, 80);
    assert_eq!(app.views[&view].media.rate, 1.0);

    // The frontend reports duration and progress; progress alone is not worth
    // a checkpoint, and a paused player cannot move the playhead.
    app.dispatch(Command::MediaPlayback {
        pane: app.focus,
        position_ms: 400,
        duration_ms: 1000,
        ended: false,
        error: None,
    });
    assert_eq!(app.views[&view].media.duration_ms, 1000);
    assert_eq!(app.views[&view].media.position_ms, 0);

    control(&mut app, MediaControl::Play);
    assert!(app.views[&view].media.playing);
    app.dispatch(Command::MediaPlayback {
        pane: app.focus,
        position_ms: 400,
        duration_ms: 1000,
        ended: false,
        error: None,
    });
    assert_eq!(app.views[&view].media.position_ms, 400);
    control(&mut app, MediaControl::Pause);
    assert!(!app.views[&view].media.playing);
    control(&mut app, MediaControl::Toggle);
    assert!(app.views[&view].media.playing);

    app.dispatch(Command::MediaVolume {
        pane: app.focus,
        volume: 250,
    });
    assert_eq!(app.views[&view].media.volume, 100);
    app.dispatch(Command::MediaVolume {
        pane: app.focus,
        volume: 35,
    });
    assert_eq!(app.views[&view].media.volume, 35);
    app.dispatch(Command::MediaRate {
        pane: app.focus,
        rate: 99.0,
    });
    assert_eq!(app.views[&view].media.rate, 4.0);
    app.dispatch(Command::MediaRate {
        pane: app.focus,
        rate: 0.75,
    });
    assert_eq!(app.views[&view].media.rate, 0.75);
    app.dispatch(Command::MediaSeek {
        pane: app.focus,
        position_ms: 5000,
    });
    assert_eq!(
        app.views[&view].media.position_ms, 1000,
        "Seeking stops at the duration"
    );
    app.dispatch(Command::MediaMute {
        pane: app.focus,
        muted: None,
    });
    assert!(app.views[&view].media.muted);
    app.dispatch(Command::MediaLoop {
        pane: app.focus,
        loop_playback: None,
    });
    assert!(app.views[&view].media.loop_playback);

    control(&mut app, MediaControl::Stop);
    assert!(!app.views[&view].media.playing);
    assert_eq!(app.views[&view].media.position_ms, 0);
    // A progress report already in flight must not undo the rewind.
    app.dispatch(Command::MediaPlayback {
        pane: app.focus,
        position_ms: 780,
        duration_ms: 1000,
        ended: false,
        error: None,
    });
    assert_eq!(app.views[&view].media.position_ms, 0);

    // A playback error clears the playing intent and is shown on the tab.
    app.dispatch(Command::MediaControl {
        pane: app.focus,
        control: MediaControl::Play,
    });
    app.dispatch(Command::MediaPlayback {
        pane: app.focus,
        position_ms: 0,
        duration_ms: 0,
        ended: false,
        error: Some("No decoder for this format".into()),
    });
    assert!(!app.views[&view].media.playing);
    assert_eq!(
        app.views[&view].media.error.as_deref(),
        Some("No decoder for this format")
    );
    // Ending the file rewinds it.
    app.dispatch(Command::MediaPlayback {
        pane: app.focus,
        position_ms: 900,
        duration_ms: 1000,
        ended: true,
        error: None,
    });
    assert_eq!(app.views[&view].media.position_ms, 0);
}

#[test]
fn transport_actions_only_apply_to_focused_audio_tabs() {
    let (dir, _guard) = common::isolated();
    let path = copy(dir.path(), "tone.ogg");
    let mut app = App::launch(&launch(dir.path()), None).unwrap();
    // A text document cannot be played.
    app.dispatch(Command::InvokeAction {
        id: "media-toggle".into(),
        argument: String::new(),
    });
    assert_eq!(app.status, "Error: Focus an audio tab");
    let _ = open(&mut app, &path);
    app.dispatch(Command::InvokeAction {
        id: "media-toggle".into(),
        argument: String::new(),
    });
    assert!(app.views[&active(&app)].media.playing);
    app.dispatch(Command::InvokeAction {
        id: "media-volume-down".into(),
        argument: String::new(),
    });
    assert_eq!(app.views[&active(&app)].media.volume, 75);
    // The terminal frontend offers no transport actions.
    let catalog = app.command_catalog_for("audio", app.focus, None);
    assert!(catalog.iter().any(|c| c.id == "media-toggle" && c.enabled));
}

#[test]
fn recovery_restores_transport_settings_and_never_autoplays() {
    let (dir, _guard) = common::isolated();
    let path = copy(dir.path(), "tone.flac");
    let mut request = launch(dir.path());
    request.files = vec![slate_core::cli::LaunchFile {
        path: path.clone(),
        ..Default::default()
    }];
    let mut app = cli::start(&request).unwrap();
    let id = app.views[&active(&app)].document;
    app.dispatch(Command::MediaVolume {
        pane: app.focus,
        volume: 25,
    });
    app.dispatch(Command::MediaLoop {
        pane: app.focus,
        loop_playback: Some(true),
    });
    app.dispatch(Command::MediaPlayback {
        pane: app.focus,
        position_ms: 600,
        duration_ms: 1000,
        ended: false,
        error: None,
    });
    app.dispatch(Command::MediaSeek {
        pane: app.focus,
        position_ms: 600,
    });
    control(&mut app, MediaControl::Play);
    app.flush_workspace().unwrap();
    let view = active(&app);
    let cursor_view = app.views[&view].clone();
    drop(app);

    let mut app = cli::start(&request).unwrap();
    let restored = app
        .views
        .values()
        .find(|v| v.document == id)
        .unwrap_or_else(|| panic!("the audio tab was not recovered"));
    assert_eq!(restored.media.volume, 25);
    assert!(restored.media.loop_playback);
    assert_eq!(restored.media.position_ms, 600);
    assert!(!restored.media.playing, "Recovery never starts audio");
    assert!(restored.media.error.is_none());
    assert!(app.documents[&id].media.is_some());
    assert_eq!(cursor_view.document, id);
    app.flush_workspace().unwrap();
}

#[test]
fn terminal_recovery_shows_where_audio_plays() {
    let (dir, _guard) = common::isolated();
    let path = copy(dir.path(), "tone.m4a");
    let mut request = launch(dir.path());
    let mut app = cli::start(&request).unwrap();
    let id = open(&mut app, &path);
    app.flush_workspace().unwrap();
    drop(app);

    request.frontend = Some(Frontend::Tui);
    let mut terminal = cli::start(&request).unwrap();
    assert!(terminal.documents[&id].media.is_some());
    let frame = terminal.snapshot(100, 40, 0, 1, 1, 1);
    let screen = frame
        .panes
        .iter()
        .find(|p| p.focused)
        .and_then(|p| p.screen.as_ref())
        .expect("the terminal draws a message instead of a player");
    let message: String = screen.cells[0].iter().map(|c| c.text.as_str()).collect();
    assert!(message.contains("slate-gui"), "{message}");
    // The transport actions stay unavailable in the terminal frontend.
    let catalog = terminal.command_catalog_for("audio", terminal.focus, None);
    assert!(catalog.iter().all(|c| c.id != "media-toggle" || !c.enabled));
    terminal.flush_workspace().unwrap();
}
