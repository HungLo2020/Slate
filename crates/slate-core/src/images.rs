//! Image tabs retain a file reference, never binary data in a text buffer.
use crate::{document::Document, layout::View, App};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct ImageFile {
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageViewport {
    pub fit: bool,
    pub zoom: f64,
    pub x: f64,
    pub y: f64,
}
impl Default for ImageViewport {
    fn default() -> Self {
        Self {
            fit: true,
            zoom: 1.0,
            x: 0.0,
            y: 0.0,
        }
    }
}
impl ImageViewport {
    pub fn valid(&self) -> bool {
        self.zoom.is_finite()
            && (0.1..=8.0).contains(&self.zoom)
            && self.x.is_finite()
            && self.y.is_finite()
            && (0.0..=8_000_000.0).contains(&self.x)
            && (0.0..=8_000_000.0).contains(&self.y)
    }
}

#[derive(Serialize)]
pub struct ImagePresentation {
    pub document: u64,
    pub path: String,
    pub bytes: u64,
    pub generation: u64,
    pub viewport: ImageViewport,
    pub source_available: bool,
    pub unavailable: bool,
}

pub fn image_extension(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "jfif"
                | "svg"
                | "svgz"
                | "gif"
                | "bmp"
                | "webp"
                | "ico"
                | "cur"
                | "tif"
                | "tiff"
                | "avif"
                | "heic"
                | "heif"
                | "jxl"
                | "pbm"
                | "pgm"
                | "ppm"
                | "pnm"
                | "xbm"
                | "xpm"
                | "tga"
                | "qoi"
                | "icns"
                | "psd"
                | "exr"
                | "hdr"
        )
    })
}

pub fn svg(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("svg"))
}

impl Document {
    /// A small header probe also recognizes common attachments without an
    /// extension. Audio is checked first: its extensions never overlap images.
    pub fn open_for_frontend(path: &Path, attachments: bool) -> Result<Self> {
        if attachments {
            let (mut file, _) = crate::fsio::open_regular(path)?;
            let mut header = [0u8; 16];
            let n = file.read(&mut header)?;
            let h = &header[..n];
            if crate::media::media_extension(path) || crate::media::looks_like_media(h) {
                return Self::open_media(path);
            }
            if image_extension(path)
                || h.starts_with(b"\x89PNG\r\n\x1a\n")
                || h.starts_with(b"\xff\xd8\xff")
                || h.starts_with(b"GIF87a")
                || h.starts_with(b"GIF89a")
                || (h.starts_with(b"RIFF") && h.get(8..12) == Some(b"WEBP"))
            {
                return Self::open_image(path);
            }
        }
        Self::open(path)
    }

    /// Reopen a changed attachment from disk, keeping audio and images apart.
    pub(crate) fn open_attachment(path: &Path) -> Result<Self> {
        let (mut file, _) = crate::fsio::open_regular(path)?;
        let mut header = [0u8; 16];
        let n = file.read(&mut header)?;
        if crate::media::media_extension(path) || crate::media::looks_like_media(&header[..n]) {
            Self::open_media(path)
        } else {
            Self::open_image(path)
        }
    }

    pub(crate) fn open_image(path: &Path) -> Result<Self> {
        let path = path.canonicalize().context("Cannot resolve image file")?;
        let (_, meta) = crate::fsio::open_regular(&path)?;
        anyhow::ensure!(
            meta.len() <= MAX_IMAGE_BYTES,
            "Image files are limited to 64 MiB"
        );
        let mut doc = Self::scratch();
        doc.image = Some(ImageFile { bytes: meta.len() });
        doc.read_only = true;
        doc.stamp = crate::document::FileStamp::of(&path);
        doc.path = Some(path);
        Ok(doc)
    }
}

impl App {
    pub(crate) fn image_viewport(&mut self, pane: u64, viewport: ImageViewport) -> Result<()> {
        anyhow::ensure!(viewport.valid(), "Invalid image viewport");
        let Some(View::Editor(id)) = self.layout.view(pane) else {
            anyhow::bail!("Focus an image first");
        };
        let v = self.views.get_mut(id).context("Missing image view")?;
        anyhow::ensure!(
            self.documents[&v.document].image.is_some(),
            "Focus an image first"
        );
        v.image_viewport = viewport;
        self.recovery.dirty = true;
        Ok(())
    }

    pub(crate) fn open_image_source(&mut self) -> Result<()> {
        let id = self.active_editor().context("Focus an SVG image first")?;
        let doc = &self.documents[&self.views[&id].document];
        let path = doc.path.clone().context("Image has no file")?;
        anyhow::ensure!(
            doc.image.is_some() && svg(&path),
            "Open as Text is available for SVG images"
        );
        self.queue_open_as(path, false)?;
        Ok(())
    }
}
