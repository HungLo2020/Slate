//! On-disk text representation. Buffers always use `\n` internally; the
//! encoding, byte-order mark and line-ending style are restored on save.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
    Cr,
}
impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
            Self::Cr => "\r",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
            Self::Cr => "CR",
        }
    }
}
impl std::str::FromStr for LineEnding {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "lf" | "unix" => Ok(Self::Lf),
            "crlf" | "dos" | "windows" => Ok(Self::Crlf),
            "cr" | "mac" => Ok(Self::Cr),
            _ => bail!("Line ending must be lf, crlf or cr"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextFormat {
    /// An encoding_rs canonical name, such as `UTF-8` or `windows-1252`.
    pub encoding: String,
    pub bom: bool,
    pub line_ending: LineEnding,
    /// The file used more than one line-ending style when it was read.
    #[serde(default)]
    pub mixed_line_endings: bool,
}
impl Default for TextFormat {
    fn default() -> Self {
        Self {
            encoding: "UTF-8".into(),
            bom: false,
            line_ending: LineEnding::Lf,
            mixed_line_endings: false,
        }
    }
}
impl TextFormat {
    pub fn label(&self) -> String {
        format!(
            "{}{} · {}",
            self.encoding,
            if self.bom { " BOM" } else { "" },
            self.line_ending.label()
        )
    }
}

pub struct Decoded {
    pub text: String,
    pub format: TextFormat,
    pub notice: Option<String>,
}

/// Resolve a user-supplied encoding label to its canonical name.
pub fn encoding_name(label: &str) -> Result<String> {
    let label = label.trim();
    let normalized = label.to_ascii_lowercase().replace('_', "-");
    if matches!(
        normalized.as_str(),
        "utf-16" | "utf16" | "utf-16le" | "utf16le"
    ) {
        return Ok("UTF-16LE".into());
    }
    if matches!(normalized.as_str(), "utf-16be" | "utf16be") {
        return Ok("UTF-16BE".into());
    }
    if matches!(normalized.as_str(), "latin1" | "latin-1" | "iso-8859-1") {
        return Ok("ISO-8859-1".into());
    }
    let encoding = encoding_rs::Encoding::for_label(normalized.as_bytes())
        .with_context(|| format!("Unknown encoding: {label}"))?;
    if encoding == encoding_rs::REPLACEMENT || encoding == encoding_rs::X_USER_DEFINED {
        bail!("Unsupported encoding: {label}");
    }
    Ok(encoding.name().into())
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(64 * 1024)].contains(&0)
}

/// Decode file contents. `forced` reinterprets the bytes with a chosen encoding.
pub fn decode(bytes: &[u8], forced: Option<&str>) -> Result<Decoded> {
    let mut notice = None;
    let (encoding, bom, body): (String, bool, &[u8]) = if let Some(label) = forced {
        let name = encoding_name(label)?;
        let (bom, body) = strip_bom(bytes, &name);
        (name, bom, body)
    } else if let Some(body) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        ("UTF-8".into(), true, body)
    } else if let Some(body) = bytes.strip_prefix(b"\xFF\xFE") {
        ("UTF-16LE".into(), true, body)
    } else if let Some(body) = bytes.strip_prefix(b"\xFE\xFF") {
        ("UTF-16BE".into(), true, body)
    } else if looks_binary(bytes) {
        bail!("This looks like a binary file (it contains NUL bytes); Slate edits text files");
    } else if std::str::from_utf8(bytes).is_ok() {
        ("UTF-8".into(), false, bytes)
    } else {
        notice = Some("Not valid UTF-8; opened as windows-1252 (Latin-1)".to_string());
        ("windows-1252".into(), false, bytes)
    };
    let text = decode_body(body, &encoding)?;
    let (text, line_ending, mixed) = normalize(&text);
    if mixed {
        notice.get_or_insert_with(|| {
            format!("Mixed line endings; saving uses {}", line_ending.label())
        });
    }
    Ok(Decoded {
        text: text.into_owned(),
        format: TextFormat {
            encoding,
            bom,
            line_ending,
            mixed_line_endings: mixed,
        },
        notice,
    })
}

fn strip_bom<'a>(bytes: &'a [u8], encoding: &str) -> (bool, &'a [u8]) {
    let bom: &[u8] = match encoding {
        "UTF-8" => b"\xEF\xBB\xBF",
        "UTF-16LE" => b"\xFF\xFE",
        "UTF-16BE" => b"\xFE\xFF",
        _ => return (false, bytes),
    };
    match bytes.strip_prefix(bom) {
        Some(body) => (true, body),
        None => (false, bytes),
    }
}

fn decode_body(body: &[u8], encoding: &str) -> Result<String> {
    match encoding {
        "ISO-8859-1" => Ok(body.iter().map(|b| char::from(*b)).collect()),
        "UTF-8" => Ok(std::str::from_utf8(body)
            .context("The file is not valid UTF-8")?
            .to_owned()),
        "UTF-16LE" | "UTF-16BE" => {
            if !body.len().is_multiple_of(2) {
                bail!("The file has an odd number of bytes for {encoding}");
            }
            let units = body.as_chunks::<2>().0.iter().map(|pair| {
                if encoding == "UTF-16LE" {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            });
            char::decode_utf16(units)
                .collect::<Result<String, _>>()
                .with_context(|| format!("The file is not valid {encoding}"))
        }
        name => {
            let encoding = encoding_rs::Encoding::for_label(name.as_bytes())
                .with_context(|| format!("Unknown encoding: {name}"))?;
            encoding
                .decode_without_bom_handling_and_without_replacement(body)
                .map(Cow::into_owned)
                .with_context(|| format!("The file is not valid {name}"))
        }
    }
}

/// Convert every line ending to `\n` and report the dominant original style.
pub fn normalize(text: &str) -> (Cow<'_, str>, LineEnding, bool) {
    let bytes = text.as_bytes();
    if !bytes.contains(&b'\r') {
        return (Cow::Borrowed(text), LineEnding::Lf, false);
    }
    let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                crlf += 1;
                out.push('\n');
            }
            '\r' => {
                cr += 1;
                out.push('\n');
            }
            '\n' => {
                lf += 1;
                out.push('\n');
            }
            c => out.push(c),
        }
    }
    let kinds = [lf, crlf, cr].iter().filter(|n| **n > 0).count();
    let ending = if crlf >= lf && crlf >= cr {
        LineEnding::Crlf
    } else if cr > lf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    };
    (Cow::Owned(out), ending, kinds > 1)
}

/// Normalize text arriving from paste, IME or other programs to `\n` lines.
pub fn normalize_input(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(normalize(text).0.into_owned())
    } else {
        Cow::Borrowed(text)
    }
}

/// Validate representability without flattening or allocating the encoded document.
pub fn validate_rope(text: &ropey::Rope, format: &TextFormat) -> Result<()> {
    encode_chunks(text, format, |_| Ok(()))
}
/// Encode chunks with one streaming encoder, bounding output to the document limit.
pub fn encode_rope(text: &ropey::Rope, format: &TextFormat) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len_bytes().min(16 * 1024 * 1024));
    encode_chunks(text, format, |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(bytes)
}
fn encode_chunks(
    text: &ropey::Rope,
    format: &TextFormat,
    mut emit: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut total = 0usize;
    let mut output = |bytes: &[u8]| -> Result<()> {
        total = total
            .checked_add(bytes.len())
            .context("Encoded size overflow")?;
        anyhow::ensure!(
            total <= crate::document::MAX_FILE_BYTES,
            "Encoded document exceeds 1 GiB"
        );
        emit(bytes)
    };
    let unicode = matches!(
        format.encoding.as_str(),
        "UTF-8" | "UTF-16LE" | "UTF-16BE" | "ISO-8859-1"
    );
    if unicode {
        if format.bom {
            match format.encoding.as_str() {
                "UTF-8" => output(b"\xef\xbb\xbf")?,
                "UTF-16LE" => output(b"\xff\xfe")?,
                "UTF-16BE" => output(b"\xfe\xff")?,
                _ => {}
            }
        }
        let mut chunk_format = format.clone();
        chunk_format.bom = false;
        for chunk in text.chunks() {
            output(&encode(chunk, &chunk_format)?)?;
        }
        return Ok(());
    }
    let encoding =
        encoding_rs::Encoding::for_label(format.encoding.as_bytes()).context("Unknown encoding")?;
    let mut encoder = encoding.new_encoder();
    let mut buffer = [0u8; 8192];
    let mut write = |input: &str, last: bool| -> Result<()> {
        let mut rest = input;
        loop {
            let (result, read, written) =
                encoder.encode_from_utf8_without_replacement(rest, &mut buffer, last);
            output(&buffer[..written])?;
            rest = &rest[read..];
            match result {
                encoding_rs::EncoderResult::InputEmpty => return Ok(()),
                encoding_rs::EncoderResult::OutputFull => {}
                encoding_rs::EncoderResult::Unmappable(c) => bail!(
                    "{} cannot store '{c}' (U+{:04X})",
                    format.encoding,
                    c as u32
                ),
            }
        }
    };
    for chunk in text.chunks() {
        if format.line_ending == LineEnding::Lf {
            write(chunk, false)?;
        } else {
            write(&chunk.replace('\n', format.line_ending.as_str()), false)?;
        }
    }
    write("", true)
}

pub fn encode(text: &str, format: &TextFormat) -> Result<Vec<u8>> {
    let text: Cow<str> = if format.line_ending == LineEnding::Lf {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.replace('\n', format.line_ending.as_str()))
    };
    let mut bytes = Vec::with_capacity(text.len() + 3);
    match format.encoding.as_str() {
        "ISO-8859-1" => {
            for c in text.chars() {
                bytes.push(
                    u8::try_from(c as u32)
                        .with_context(|| format!("ISO-8859-1 cannot store '{c}'"))?,
                );
            }
        }
        "UTF-8" => {
            if format.bom {
                bytes.extend_from_slice(b"\xEF\xBB\xBF");
            }
            bytes.extend_from_slice(text.as_bytes());
        }
        "UTF-16LE" | "UTF-16BE" => {
            let little = format.encoding == "UTF-16LE";
            if format.bom {
                bytes.extend_from_slice(if little { b"\xFF\xFE" } else { b"\xFE\xFF" });
            }
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
        }
        name => {
            let encoding = encoding_rs::Encoding::for_label(name.as_bytes())
                .with_context(|| format!("Unknown encoding: {name}"))?;
            let mut encoder = encoding.new_encoder();
            let mut output = vec![
                0;
                encoder
                    .max_buffer_length_from_utf8_without_replacement(text.len())
                    .context("Document is too large to encode")?
            ];
            let (result, _, written) =
                encoder.encode_from_utf8_without_replacement(&text, &mut output, true);
            match result {
                encoding_rs::EncoderResult::InputEmpty => {}
                encoding_rs::EncoderResult::Unmappable(c) => bail!(
                    "{name} cannot store the character '{c}' (U+{:04X}). Change the encoding, for example: set-encoding utf-8",
                    c as u32
                ),
                encoding_rs::EncoderResult::OutputFull => bail!("Encoding buffer overflow"),
            }
            output.truncate(written);
            bytes = output;
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_round_trip_and_report_mixed_files() {
        let decoded = decode(b"a\r\nb\r\nc", None).unwrap();
        assert_eq!(decoded.text, "a\nb\nc");
        assert_eq!(decoded.format.line_ending, LineEnding::Crlf);
        assert!(!decoded.format.mixed_line_endings);
        assert_eq!(
            encode(&decoded.text, &decoded.format).unwrap(),
            b"a\r\nb\r\nc"
        );
        let mixed = decode(b"a\r\nb\nc\r\n", None).unwrap();
        assert!(mixed.format.mixed_line_endings);
        assert_eq!(mixed.format.line_ending, LineEnding::Crlf);
        assert!(mixed.notice.is_some());
        let mac = decode(b"a\rb\r", None).unwrap();
        assert_eq!(
            (mac.text.as_str(), mac.format.line_ending),
            ("a\nb\n", LineEnding::Cr)
        );
    }

    #[test]
    fn boms_and_legacy_encodings_are_preserved() {
        let utf8 = decode(b"\xEF\xBB\xBFhi\n", None).unwrap();
        assert_eq!((utf8.text.as_str(), utf8.format.bom), ("hi\n", true));
        assert_eq!(
            encode(&utf8.text, &utf8.format).unwrap(),
            b"\xEF\xBB\xBFhi\n"
        );
        let utf16 = decode(b"\xFF\xFEh\0i\0\n\0", None).unwrap();
        assert_eq!(utf16.text, "hi\n");
        assert_eq!(utf16.format.encoding, "UTF-16LE");
        assert_eq!(
            encode(&utf16.text, &utf16.format).unwrap(),
            b"\xFF\xFEh\0i\0\n\0"
        );
        let latin = decode(b"caf\xe9\n", None).unwrap();
        assert_eq!(latin.text, "café\n");
        assert_eq!(latin.format.encoding, "windows-1252");
        assert!(latin.notice.is_some());
        assert_eq!(encode(&latin.text, &latin.format).unwrap(), b"caf\xe9\n");
        let error = encode("€ ✓", &latin.format).unwrap_err().to_string();
        assert!(error.contains("cannot store"), "{error}");
        let forced = decode(b"caf\xc3\xa9", Some("latin1")).unwrap();
        assert_eq!(forced.text, "cafÃ©");
    }

    #[test]
    fn binary_files_are_refused_and_input_is_normalized() {
        assert!(decode(b"ELF\0\x01\x02", None).is_err());
        assert_eq!(normalize_input("a\rb\r\nc"), "a\nb\nc");
        assert!(matches!(normalize_input("plain"), Cow::Borrowed(_)));
        assert_eq!(encoding_name("latin1").unwrap(), "ISO-8859-1");
        assert_eq!(encoding_name("utf16").unwrap(), "UTF-16LE");
        assert!(encoding_name("klingon").is_err());
    }
}
