//! Audio tabs retain a file reference, never audio data in a text buffer.
//!
//! Decoding is deliberately not part of the core: the desktop frontend plays a
//! tab through Qt Multimedia when that module is installed, and the core owns
//! the transport state, commands and recovery. Video is not supported yet.
use crate::{document::Document, layout::View, App};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Audio files are streamed by the frontend, never read into memory.
pub const MAX_MEDIA_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Extensions the desktop editor opens in an audio tab.
const EXTENSIONS: &[&str] = &[
    "mp3", "ogg", "oga", "opus", "wav", "flac", "m4a", "m4b", "aac", "aiff", "aif", "aifc", "wma",
    "caf",
];

#[derive(Clone, Serialize, Deserialize)]
pub struct MediaFile {
    pub bytes: u64,
}

/// Transport state, kept in the core so the palette, both frontends and
/// recovery agree. `playing` and `error` describe this session only and are
/// never written to a checkpoint: reopening a workspace must not start audio.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaState {
    pub volume: u8,
    pub muted: bool,
    pub loop_playback: bool,
    pub rate: f32,
    pub position_ms: u64,
    pub duration_ms: u64,
    #[serde(skip)]
    pub playing: bool,
    #[serde(skip)]
    pub error: Option<String>,
}
impl Default for MediaState {
    fn default() -> Self {
        Self {
            volume: 80,
            muted: false,
            loop_playback: false,
            rate: 1.0,
            position_ms: 0,
            duration_ms: 0,
            playing: false,
            error: None,
        }
    }
}
impl MediaState {
    /// Keep a recovered or externally supplied state inside what the player
    /// accepts, so a hand-edited checkpoint cannot ask for a silent or
    /// distorted tab.
    pub fn clamp(&mut self) {
        self.volume = self.volume.min(100);
        if !self.rate.is_finite() {
            self.rate = 1.0;
        }
        self.rate = self.rate.clamp(MIN_RATE, MAX_RATE);
        if self.duration_ms > 0 {
            self.position_ms = self.position_ms.min(self.duration_ms);
        }
    }
}

pub const MIN_RATE: f32 = 0.25;
pub const MAX_RATE: f32 = 4.0;

/// A transport verb, so the palette, keys and the player UI share one path.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaControl {
    Play,
    Pause,
    Toggle,
    Stop,
}

#[derive(Serialize)]
pub struct MediaPresentation {
    pub document: u64,
    pub path: String,
    pub bytes: u64,
    pub generation: u64,
    pub volume: u8,
    pub muted: bool,
    pub loop_playback: bool,
    pub rate: f32,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub playing: bool,
    pub error: Option<String>,
    pub unavailable: bool,
}

pub fn media_extension(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        let lower = s.to_ascii_lowercase();
        EXTENSIONS.contains(&lower.as_str())
    })
}

/// A small header probe recognizes common audio containers without an
/// extension, the way image detection works.
pub fn looks_like_media(header: &[u8]) -> bool {
    if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE")
        || header.starts_with(b"OggS")
        || header.starts_with(b"fLaC")
        || header.starts_with(b"ID3")
        || (header.starts_with(b"FORM") && matches!(header.get(8..12), Some(b"AIFF" | b"AIFC")))
    {
        return true;
    }
    // MPEG-4 audio brands (`ftyp` at offset 4) and MPEG audio frame sync.
    // Video brands (mp41/mp42/isom) are deliberately not audio: video playback
    // is not supported yet, so those files stay binary.
    if header.len() >= 12 && &header[4..8] == b"ftyp" {
        return matches!(&header[8..12], b"M4A " | b"M4B " | b"M4P ");
    }
    header.len() >= 2 && header[0] == 0xFF && header[1] & 0xE0 == 0xE0
}

impl Document {
    pub(crate) fn open_media(path: &Path) -> Result<Self> {
        let path = path.canonicalize().context("Cannot resolve audio file")?;
        let (_, meta) = crate::fsio::open_regular(&path)?;
        anyhow::ensure!(
            meta.len() <= MAX_MEDIA_BYTES,
            "Audio files are limited to 2 GiB"
        );
        let mut doc = Self::scratch();
        doc.media = Some(MediaFile { bytes: meta.len() });
        doc.read_only = true;
        doc.stamp = crate::document::FileStamp::of(&path);
        doc.path = Some(path);
        Ok(doc)
    }
}

impl App {
    /// Frames carry media state, so every change must invalidate them: the
    /// GUI asks for increments and would otherwise never see a new transport
    /// state when nothing else in the workspace changed.
    fn media_touched(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    pub(crate) fn media_control(&mut self, pane: u64, control: MediaControl) -> Result<()> {
        match control {
            MediaControl::Play => self.media_play(pane, Some(true)),
            MediaControl::Pause => self.media_play(pane, Some(false)),
            MediaControl::Toggle => self.media_play(pane, None),
            MediaControl::Stop => self.media_stop(pane),
        }
    }
    /// A transport request from the palette, a key binding or the player UI.
    pub(crate) fn media_play(&mut self, pane: u64, playing: Option<bool>) -> Result<()> {
        let view = self.media_view(pane)?;
        let state = &mut self
            .views
            .get_mut(&view)
            .context("Missing media view")?
            .media;
        state.playing = playing.unwrap_or(!state.playing);
        if state.playing {
            state.error = None;
        }
        self.media_touched();
        Ok(())
    }
    pub(crate) fn media_stop(&mut self, pane: u64) -> Result<()> {
        let view = self.media_view(pane)?;
        let state = &mut self
            .views
            .get_mut(&view)
            .context("Missing media view")?
            .media;
        state.playing = false;
        state.position_ms = 0;
        self.recovery.dirty = true;
        self.media_touched();
        Ok(())
    }
    pub(crate) fn media_seek(&mut self, pane: u64, position_ms: u64) -> Result<()> {
        let view = self.media_view(pane)?;
        let state = &mut self
            .views
            .get_mut(&view)
            .context("Missing media view")?
            .media;
        state.position_ms = match state.duration_ms {
            0 => position_ms,
            duration => position_ms.min(duration),
        };
        self.recovery.dirty = true;
        self.media_touched();
        Ok(())
    }
    pub(crate) fn media_volume(&mut self, pane: u64, volume: u8) -> Result<()> {
        self.media_update(pane, |state| state.volume = volume.min(100))
    }
    pub(crate) fn media_rate(&mut self, pane: u64, rate: f32) -> Result<()> {
        anyhow::ensure!(rate.is_finite(), "Invalid playback rate");
        self.media_update(pane, |state| state.rate = rate.clamp(MIN_RATE, MAX_RATE))
    }
    pub(crate) fn media_mute(&mut self, pane: u64, muted: Option<bool>) -> Result<()> {
        self.media_update(pane, |state| state.muted = muted.unwrap_or(!state.muted))
    }
    pub(crate) fn media_loop(&mut self, pane: u64, looping: Option<bool>) -> Result<()> {
        self.media_update(pane, |state| {
            state.loop_playback = looping.unwrap_or(!state.loop_playback)
        })
    }
    /// Playback progress reported by the frontend. Only transport settings
    /// dirty recovery: position arrives twice a second while audio plays.
    pub(crate) fn media_playback(
        &mut self,
        pane: u64,
        position_ms: u64,
        duration_ms: u64,
        ended: bool,
        error: Option<String>,
    ) -> Result<()> {
        let view = self.media_view(pane)?;
        let state = &mut self
            .views
            .get_mut(&view)
            .context("Missing media view")?
            .media;
        state.duration_ms = duration_ms;
        // Progress is only authoritative while the core believes audio plays:
        // a report can arrive after a Stop or a seek and must not undo it.
        if state.playing {
            state.position_ms = position_ms.min(duration_ms);
        }
        if ended {
            state.playing = false;
            state.position_ms = 0;
        }
        if let Some(error) = error {
            state.error = Some(error);
            state.playing = false;
        }
        self.media_touched();
        Ok(())
    }
    fn media_update(&mut self, pane: u64, change: impl FnOnce(&mut MediaState)) -> Result<()> {
        let view = self.media_view(pane)?;
        change(
            &mut self
                .views
                .get_mut(&view)
                .context("Missing media view")?
                .media,
        );
        self.recovery.dirty = true;
        self.media_touched();
        Ok(())
    }
    /// The editor view of an audio tab in `pane`.
    pub(crate) fn media_view(&self, pane: u64) -> Result<u64> {
        let Some(View::Editor(id)) = self.layout.view(pane) else {
            anyhow::bail!("Focus an audio tab first");
        };
        anyhow::ensure!(
            self.documents[&self.views[id].document].media.is_some(),
            "Focus an audio tab first"
        );
        Ok(*id)
    }
}
