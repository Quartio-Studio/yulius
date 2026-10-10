//! Demuxing of the encoder's output (SPEC-002 ADR-15).
//!
//! `ffmpeg` writes the H.264 stream as FLV on its stdout (`-f flv`, flushed after every
//! packet). FLV frames each access unit with its size, so a frame is complete the moment its
//! last byte arrives; a raw Annex-B stream would only show an access unit's end when the next
//! one starts, one frame interval (16.7 ms at 60 fps) later. Video tags carry AVC: one
//! AVCDecoderConfigurationRecord (`avcC`, the decoder configuration) and then access units as
//! length-prefixed NAL units, which WebCodecs decodes as they are.
//!
//! Bounds: a tag larger than [`MAX_TAG_BYTES`] is a protocol error (the stream is abandoned),
//! so the parser never buffers more than one tag.

/// Largest accepted tag payload (a keyframe of the largest display is a few MiB).
pub const MAX_TAG_BYTES: usize = 8 << 20;
/// FLV file header length (signature, version, flags, header size) plus the first
/// `PreviousTagSize`.
const FILE_HEADER_LEN: usize = 9 + 4;
/// Tag header length.
const TAG_HEADER_LEN: usize = 11;
/// Trailing `PreviousTagSize`.
const TAG_TRAILER_LEN: usize = 4;
const TAG_VIDEO: u8 = 9;
const CODEC_AVC: u8 = 7;
const FRAME_KEY: u8 = 1;
const AVC_SEQUENCE_HEADER: u8 = 0;
const AVC_NALU: u8 = 1;

/// One item of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlvItem {
    /// The AVCDecoderConfigurationRecord (`avcC`).
    AvcConfig(Vec<u8>),
    /// One access unit.
    Video {
        /// An IDR access unit (a decoder can start here).
        keyframe: bool,
        /// Decoding timestamp in milliseconds.
        timestamp_ms: u32,
        /// Length-prefixed NAL units.
        data: Vec<u8>,
    },
}

/// Why the stream cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlvError {
    /// The stream does not start with an FLV header.
    #[error("the encoder output is not FLV")]
    NotFlv,
    /// A tag exceeds [`MAX_TAG_BYTES`].
    #[error("encoder tag of {0} bytes exceeds the bound")]
    TagTooLarge(usize),
    /// A video tag is not H.264 or is truncated.
    #[error("unexpected video tag: {0}")]
    BadVideoTag(&'static str),
}

/// Incremental FLV parser: feed it the bytes as they arrive.
#[derive(Debug, Default)]
pub struct FlvParser {
    buffer: Vec<u8>,
    header_done: bool,
}

impl FlvParser {
    /// A parser expecting the FLV file header first.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds bytes and returns every item they complete, in order.
    ///
    /// # Errors
    /// [`FlvError`] for anything but a well-formed FLV stream of H.264 video; the parser is then
    /// unusable (the encoder is restarted).
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<FlvItem>, FlvError> {
        self.buffer.extend_from_slice(bytes);
        let mut items = Vec::new();
        let mut offset = 0;
        if !self.header_done {
            if self.buffer.len() < FILE_HEADER_LEN {
                return Ok(items);
            }
            if &self.buffer[..3] != b"FLV" {
                return Err(FlvError::NotFlv);
            }
            let header = u32::from_be_bytes([
                self.buffer[5],
                self.buffer[6],
                self.buffer[7],
                self.buffer[8],
            ]);
            let header = usize::try_from(header).map_err(|_| FlvError::NotFlv)?;
            if !(9..=64).contains(&header) {
                return Err(FlvError::NotFlv);
            }
            if self.buffer.len() < header + 4 {
                return Ok(items);
            }
            offset = header + 4;
            self.header_done = true;
        }
        loop {
            let rest = &self.buffer[offset..];
            if rest.len() < TAG_HEADER_LEN {
                break;
            }
            let size =
                usize::from(rest[1]) << 16 | usize::from(rest[2]) << 8 | usize::from(rest[3]);
            if size > MAX_TAG_BYTES {
                return Err(FlvError::TagTooLarge(size));
            }
            let total = TAG_HEADER_LEN + size + TAG_TRAILER_LEN;
            if rest.len() < total {
                break;
            }
            let timestamp = u32::from(rest[7]) << 24
                | u32::from(rest[4]) << 16
                | u32::from(rest[5]) << 8
                | u32::from(rest[6]);
            if rest[0] & 0x1F == TAG_VIDEO {
                let payload = &rest[TAG_HEADER_LEN..TAG_HEADER_LEN + size];
                if let Some(item) = video_item(payload, timestamp)? {
                    items.push(item);
                }
            }
            offset += total;
        }
        self.buffer.drain(..offset);
        Ok(items)
    }
}

fn video_item(payload: &[u8], timestamp_ms: u32) -> Result<Option<FlvItem>, FlvError> {
    let (&first, rest) = payload
        .split_first()
        .ok_or(FlvError::BadVideoTag("empty"))?;
    if first & 0x0F != CODEC_AVC {
        return Err(FlvError::BadVideoTag("not H.264"));
    }
    if rest.len() < 4 {
        return Err(FlvError::BadVideoTag("truncated AVC header"));
    }
    let data = &rest[4..];
    match rest[0] {
        AVC_SEQUENCE_HEADER => {
            if data.len() < 7 || data[0] != 1 {
                return Err(FlvError::BadVideoTag("invalid avcC"));
            }
            Ok(Some(FlvItem::AvcConfig(data.to_vec())))
        }
        AVC_NALU if !data.is_empty() => Ok(Some(FlvItem::Video {
            keyframe: first >> 4 == FRAME_KEY,
            timestamp_ms,
            data: data.to_vec(),
        })),
        // End of sequence or an empty access unit.
        _ => Ok(None),
    }
}

/// The WebCodecs codec string of an `avcC` record: `avc1.` and its profile, constraint flags
/// and level as hex (e.g. `avc1.42c028`, constrained baseline 4.0).
#[must_use]
pub fn avc_codec_string(record: &[u8]) -> Option<String> {
    match record {
        [1, profile, constraints, level, ..] => {
            Some(format!("avc1.{profile:02x}{constraints:02x}{level:02x}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn tag(kind: u8, timestamp: u32, payload: &[u8]) -> Vec<u8> {
        let size = payload.len();
        let mut tag = vec![
            kind,
            (size >> 16) as u8,
            (size >> 8) as u8,
            size as u8,
            (timestamp >> 16) as u8,
            (timestamp >> 8) as u8,
            timestamp as u8,
            (timestamp >> 24) as u8,
            0,
            0,
            0,
        ];
        tag.extend_from_slice(payload);
        tag.extend_from_slice(&u32::try_from(11 + size).unwrap_or(0).to_be_bytes());
        tag
    }

    fn header() -> Vec<u8> {
        let mut header = b"FLV\x01\x01\x00\x00\x00\x09".to_vec();
        header.extend_from_slice(&[0, 0, 0, 0]);
        header
    }

    const AVCC: [u8; 9] = [1, 0x42, 0xC0, 0x28, 0xFF, 0xE1, 0, 1, 0x67];

    fn stream() -> Vec<u8> {
        let mut bytes = header();
        bytes.extend(tag(18, 0, b"\x02\x00\x0aonMetaData"));
        bytes.extend(tag(9, 0, &[&[0x17, 0, 0, 0, 0][..], &AVCC].concat()));
        bytes.extend(tag(9, 0, &[0x17, 1, 0, 0, 0, 0, 0, 0, 2, 0x65, 0x88]));
        bytes.extend(tag(9, 17, &[0x27, 1, 0, 0, 0, 0, 0, 0, 2, 0x41, 0x9A]));
        bytes.extend(tag(9, 0x0100_0021, &[0x27, 1, 0, 0, 0, 0, 0, 0, 1, 0x41]));
        bytes
    }

    fn expected() -> Vec<FlvItem> {
        vec![
            FlvItem::AvcConfig(AVCC.to_vec()),
            FlvItem::Video {
                keyframe: true,
                timestamp_ms: 0,
                data: vec![0, 0, 0, 2, 0x65, 0x88],
            },
            FlvItem::Video {
                keyframe: false,
                timestamp_ms: 17,
                data: vec![0, 0, 0, 2, 0x41, 0x9A],
            },
            FlvItem::Video {
                keyframe: false,
                timestamp_ms: 0x0100_0021,
                data: vec![0, 0, 0, 1, 0x41],
            },
        ]
    }

    #[test]
    fn a_whole_stream_yields_the_config_and_each_access_unit() -> TestResult {
        let mut parser = FlvParser::new();
        assert_eq!(parser.push(&stream())?, expected());
        assert!(parser.buffer.is_empty());
        Ok(())
    }

    #[test]
    fn any_split_of_the_stream_yields_the_same_items_as_soon_as_complete() -> TestResult {
        let bytes = stream();
        for chunk in [1, 2, 3, 7, 13, 64] {
            let mut parser = FlvParser::new();
            let mut items = Vec::new();
            for piece in bytes.chunks(chunk) {
                items.extend(parser.push(piece)?);
            }
            assert_eq!(items, expected(), "chunk {chunk}");
        }
        // A frame is emitted with its last byte, not when the next one starts.
        let mut parser = FlvParser::new();
        let end_of_first_frame = header().len() + (11 + 13 + 4) + (11 + 14 + 4) + (11 + 11 + 4);
        let items = parser.push(&bytes[..end_of_first_frame])?;
        assert_eq!(items.len(), 2, "config and the keyframe");
        Ok(())
    }

    #[test]
    fn garbage_oversized_and_foreign_codecs_are_errors() {
        assert_eq!(
            FlvParser::new().push(b"\x00\x00\x00\x01\x67\x42\x00\x1f\x00\x00\x00\x00\x00"),
            Err(FlvError::NotFlv)
        );
        let mut huge = header();
        huge.extend_from_slice(&[9, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            FlvParser::new().push(&huge),
            Err(FlvError::TagTooLarge(0x00FF_FFFF))
        );
        let mut vp8 = header();
        vp8.extend(tag(9, 0, &[0x14, 1, 0, 0, 0]));
        assert_eq!(
            FlvParser::new().push(&vp8),
            Err(FlvError::BadVideoTag("not H.264"))
        );
        let mut bad_config = header();
        bad_config.extend(tag(9, 0, &[0x17, 0, 0, 0, 0, 2]));
        assert!(FlvParser::new().push(&bad_config).is_err());
    }

    #[test]
    fn codec_strings_follow_the_avcc_record() {
        assert_eq!(avc_codec_string(&AVCC).as_deref(), Some("avc1.42c028"));
        assert_eq!(
            avc_codec_string(&[1, 0x64, 0, 0x2A, 0xFF]).as_deref(),
            Some("avc1.64002a")
        );
        assert_eq!(avc_codec_string(&[0, 0x42, 0, 0x1F]), None);
        assert_eq!(avc_codec_string(&[1, 0x42]), None);
    }
}
