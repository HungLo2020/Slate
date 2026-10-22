mod common;
use slate_core::{
    cli::{self, Frontend, Launch, LaunchFile},
    document::Document,
    images::{ImageViewport, MAX_IMAGE_BYTES},
    preferences::StartupMode,
    App, Command,
};
use std::{fs, path::Path};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfixture\0pixels";
const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"24\"><rect width=\"32\" height=\"24\" fill=\"red\"/></svg>";

fn launch(path: &Path) -> Launch {
    Launch {
        frontend: Some(Frontend::Gui),
        directory: Some(path.into()),
        startup: Some(StartupMode::EditorOnly),
        recover: true,
        ..Default::default()
    }
}
fn open(app: &mut App, path: &Path) -> u64 {
    app.dispatch(Command::Open { path: path.into() });
    common::settle(app, "image opening", |a| {
        a.documents
            .values()
            .any(|d| d.path.as_deref() == Some(path) && d.image.is_some())
    });
    app.views[&active(app)].document
}
fn active(app: &App) -> u64 {
    match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => *id,
        _ => panic!("no active document"),
    }
}
fn action(app: &mut App, id: &str) {
    app.dispatch(Command::InvokeAction {
        id: id.into(),
        argument: String::new(),
    });
}

#[test]
fn gui_startup_opens_mixed_images_and_text_without_binary_buffers() {
    let (dir, _guard) = common::isolated();
    let png = dir.path().join("picture.PNG");
    let svg = dir.path().join("vector.svg");
    let text = dir.path().join("notes.txt");
    fs::write(&png, PNG).unwrap();
    fs::write(&svg, SVG).unwrap();
    fs::write(&text, "notes").unwrap();
    let mut request = launch(dir.path());
    request.directory = None;
    request.files = [&png, &svg, &text]
        .into_iter()
        .map(|path| LaunchFile {
            path: path.into(),
            ..Default::default()
        })
        .collect();
    let mut app = App::launch(&request, None).unwrap();
    assert!(!app.terminal_frontend);
    assert!(app.editor_only);
    assert_eq!(app.documents.len(), 3);
    assert_eq!(
        app.documents.values().filter(|d| d.image.is_some()).count(),
        2
    );
    let snapshot = app.snapshot(100, 40, 0, 1, 1, 1);
    let pane = snapshot.panes.iter().find(|p| p.focused).unwrap();
    assert_eq!(pane.kind, "image");
    assert!(pane.text.is_none() && pane.screen.is_none() && pane.editor.is_none());
    assert_eq!(pane.image.as_ref().unwrap().bytes, PNG.len() as u64);
    for d in app.documents.values_mut().filter(|d| d.image.is_some()) {
        assert!(d.read_only && d.is_empty() && !d.dirty());
        assert!(d.save(None).is_err());
        assert!(d.save(Some(&dir.path().join("do-not-create.txt"))).is_err());
        assert!(d.set_text("overwrite".into()).is_err());
        assert_eq!(
            d.recovery_copy().image.as_ref().unwrap().bytes,
            d.image.as_ref().unwrap().bytes
        );
    }
    assert!(!dir.path().join("do-not-create.txt").exists());
    assert_eq!(fs::read(&png).unwrap(), PNG);
    assert_eq!(fs::read_to_string(&svg).unwrap(), SVG);
    assert!(Document::open_for_frontend(&png, false).is_err());
    assert!(Document::open_for_frontend(&svg, false)
        .unwrap()
        .image
        .is_none());
    let extensionless = dir.path().join("no-extension");
    fs::write(&extensionless, PNG).unwrap();
    assert!(Document::open_for_frontend(&extensionless, true)
        .unwrap()
        .image
        .is_some());
}

#[test]
fn image_tabs_reuse_close_and_preserve_editable_svg_source() {
    let (dir, _guard) = common::isolated();
    let path = dir.path().join("space #% 猫.svg");
    fs::write(&path, SVG).unwrap();
    let mut app = App::launch(&launch(dir.path()), None).unwrap();
    let image = open(&mut app, &path);
    let count = app.documents.len();
    open(&mut app, &path);
    common::settle(&mut app, "reused image tab", |a| {
        a.status.starts_with("Opened")
    });
    assert_eq!(app.documents.len(), count);
    for command in ["save", "save-as", "format-document", "prompt-find", "print"] {
        if let Some(info) = app.command_info(command) {
            assert!(!info.enabled, "{command}");
        }
    }
    assert!(app.command_info("close-tab").is_none_or(|c| c.enabled));
    action(&mut app, "image-source");
    common::settle(&mut app, "SVG source", |a| {
        a.documents
            .values()
            .any(|d| d.path.as_deref() == Some(&path) && d.image.is_none())
    });
    let source = app.views[&active(&app)].document;
    assert_ne!(source, image);
    app.documents
        .get_mut(&source)
        .unwrap()
        .set_text(format!("{SVG}\n<!-- unsaved -->"))
        .unwrap();
    action(&mut app, "image-preview");
    common::settle(&mut app, "saved SVG preview", |a| {
        a.views[&active(a)].document == image
    });
    assert!(app.documents[&source].dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), SVG);
    let pane = app.focus;
    let view = active(&app);
    app.dispatch(Command::CloseTab {
        pane,
        view,
        force: false,
    });
    assert!(!app.documents.contains_key(&image));
    assert!(app.documents[&source].dirty());
}

#[test]
fn image_paths_and_viewports_restore_and_follow_disk_changes() {
    let (dir, _guard) = common::isolated();
    let path = dir.path().join("picture.png");
    fs::write(&path, PNG).unwrap();
    let request = launch(dir.path());
    let mut app = cli::start(&request).unwrap();
    let image = open(&mut app, &path);
    let view = active(&app);
    let pane = app.focus;
    app.dispatch(Command::ImageViewport {
        pane,
        viewport: ImageViewport {
            fit: false,
            zoom: 2.0,
            x: 17.0,
            y: 23.0,
        },
    });
    app.flush_workspace().unwrap();
    drop(app);
    let mut app = cli::start(&request).unwrap();
    assert_eq!(
        app.documents[&image].image.as_ref().unwrap().bytes,
        PNG.len() as u64
    );
    assert_eq!(app.views[&view].image_viewport.zoom, 2.0);
    assert_eq!(app.views[&view].image_viewport.x, 17.0);
    assert!(!app.views[&view].image_viewport.fit);
    app.dispatch(Command::ImageViewport {
        pane,
        viewport: ImageViewport {
            zoom: f64::NAN,
            ..Default::default()
        },
    });
    assert_eq!(app.views[&view].image_viewport.zoom, 2.0);
    let generation = app.documents[&image].generation;
    fs::write(&path, [PNG, b"new pixels"].concat()).unwrap();
    common::settle(&mut app, "image reload", |a| {
        a.documents[&image].generation > generation
    });
    assert_eq!(
        app.documents[&image].image.as_ref().unwrap().bytes,
        (PNG.len() + 10) as u64
    );
    fs::remove_file(&path).unwrap();
    common::settle(&mut app, "image deletion", |a| {
        a.snapshot(100, 40, 0, 1, 1, 1)
            .panes
            .iter()
            .any(|p| p.image.as_ref().is_some_and(|i| i.unavailable))
    });
    app.flush_workspace().unwrap();
    drop(app);
    let mut app = cli::start(&request).unwrap();
    assert!(app.documents[&image].image.is_some());
    fs::write(&path, PNG).unwrap();
    common::settle(&mut app, "image restored on disk", |a| {
        a.snapshot(100, 40, 0, 1, 1, 1)
            .panes
            .iter()
            .any(|p| p.image.as_ref().is_some_and(|i| !i.unavailable))
    });
}

#[test]
fn image_loading_is_bounded_and_never_opens_special_files() {
    let (dir, _guard) = common::isolated();
    let path = dir.path().join("huge.png");
    fs::File::create(&path)
        .unwrap()
        .set_len(MAX_IMAGE_BYTES + 1)
        .unwrap();
    assert!(Document::open_for_frontend(&path, true)
        .err()
        .unwrap()
        .to_string()
        .contains("64 MiB"));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let pipe = dir.path().join("pipe.png");
        let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(Document::open_for_frontend(&pipe, true).is_err());
    }
}

#[test]
fn svg_text_locations_open_source_beside_the_preview() {
    let (dir, _guard) = common::isolated();
    let path = dir.path().join("vector.svg");
    fs::write(&path, SVG).unwrap();
    let mut app = App::launch(&launch(dir.path()), None).unwrap();
    let image = open(&mut app, &path);
    app.goto(slate_core::navigation::Location {
        path: path.clone(),
        line: 0,
        column: slate_core::navigation::Column::Byte(5),
    })
    .unwrap();
    common::settle(&mut app, "SVG search location", |a| {
        a.documents
            .values()
            .any(|d| d.path.as_ref() == Some(&path) && d.image.is_none())
    });
    let view = active(&app);
    assert_ne!(app.views[&view].document, image);
    assert_eq!(app.views[&view].cursor, 5);
    assert!(!app.documents[&app.views[&view].document].read_only);
}

#[test]
fn terminal_recovery_preserves_image_tabs_without_reading_them_as_text() {
    let (dir, _guard) = common::isolated();
    let path = dir.path().join("picture.png");
    fs::write(&path, PNG).unwrap();
    let mut request = launch(dir.path());
    let mut app = cli::start(&request).unwrap();
    let image = open(&mut app, &path);
    app.flush_workspace().unwrap();
    drop(app);
    request.frontend = Some(Frontend::Tui);
    let mut terminal = cli::start(&request).unwrap();
    assert!(terminal.documents[&image].image.is_some());
    let frame = terminal.snapshot(100, 40, 0, 1, 1, 1);
    let screen = frame
        .panes
        .iter()
        .find(|p| p.focused)
        .unwrap()
        .screen
        .as_ref()
        .unwrap();
    let message: String = screen.cells[0].iter().map(|c| c.text.as_str()).collect();
    assert!(message.contains("slate-gui"));
    terminal.flush_workspace().unwrap();
    drop(terminal);
    request.frontend = Some(Frontend::Gui);
    let app = cli::start(&request).unwrap();
    assert!(app.documents[&image].image.is_some());
    assert_eq!(fs::read(path).unwrap(), PNG);
}

#[test]
fn closing_the_only_image_leaves_a_usable_empty_editor() {
    let (dir, _guard) = common::isolated();
    let path = dir.path().join("only.png");
    fs::write(&path, PNG).unwrap();
    let request = Launch {
        frontend: Some(Frontend::Gui),
        files: vec![LaunchFile {
            path: path.clone(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut app = App::launch(&request, None).unwrap();
    action(&mut app, "close");
    assert!(app.documents.values().all(|d| d.image.is_none()));
    assert!(!app.dirty());
    let frame = app.snapshot(100, 40, 0, 1, 1, 1);
    assert!(frame.panes.iter().all(|p| p.image.is_none()));
    app.dispatch(Command::New);
    let view = active(&app);
    app.documents
        .get_mut(&app.views[&view].document)
        .unwrap()
        .set_text("still editable".into())
        .unwrap();
    assert!(app.dirty());
    assert_eq!(fs::read(path).unwrap(), PNG);
}
