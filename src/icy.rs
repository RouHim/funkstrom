use bytes::Bytes;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct StatusResponse {
    pub(crate) station_name: String,
    pub(crate) station_description: String,
    pub(crate) station_genre: String,
    pub(crate) streams: Vec<StreamStatus>,
    pub(crate) uptime: String,
    pub(crate) version: String,
}

#[derive(Serialize)]
pub(crate) struct StreamStatus {
    pub(crate) name: String,
    pub(crate) bitrate: u32,
    pub(crate) status: String,
    pub(crate) buffer_chunks: usize,
    pub(crate) buffer_bytes: usize,
    pub(crate) listeners: usize,
}

// Template context structures
pub(crate) struct InfoPageContext {
    pub(crate) station_name: String,
    pub(crate) current_track: String,
    pub(crate) album: String,
    pub(crate) station_description: String,
    pub(crate) station_genre: String,
    pub(crate) bitrate: u32,
    pub(crate) public_url: String,
    pub(crate) streams: Vec<StreamLink>,
    pub(crate) first_stream: String,
    pub(crate) cover_url: String,
    pub(crate) version: String,
    pub(crate) listeners: usize,
    pub(crate) uptime: String,
}

#[derive(Serialize)]
pub(crate) struct StreamLink {
    pub(crate) name: String,
    pub(crate) bitrate: u32,
    pub(crate) url: String,
}

/// Extract cover art from an audio file's embedded tags.
/// Returns (image_bytes, mime_type) if the file has embedded cover art.
pub(crate) fn extract_cover_from_file(path: &std::path::Path) -> Option<(Vec<u8>, String)> {
    let tag = audiotags::Tag::new().read_from_path(path).ok()?;
    let album = tag.album()?;
    let cover = album.cover?;
    let mime: &'static str = cover.mime_type.into();
    Some((cover.data.to_vec(), mime.to_string()))
}
fn build_icy_metadata_block(artist_title: &str, stream_url: Option<&str>) -> Vec<u8> {
    let stream_url = stream_url.unwrap_or("");
    let stream_title = format!("StreamTitle='{}';StreamUrl='{}';", artist_title, stream_url);
    let len = stream_title.len();
    let blocks = len.div_ceil(16).min(255) as u8;
    let padded_len = blocks as usize * 16;
    let copy_len = len.min(padded_len);
    let mut block = vec![0u8; 1 + padded_len];
    block[0] = blocks;
    block[1..1 + copy_len].copy_from_slice(&stream_title.as_bytes()[..copy_len]);
    block
}

/// Process a chunk of audio data, splitting at `metaint` boundaries and inserting
/// ICY metadata blocks. Returns output blocks that are sent to the client in order.
///
/// Maintains `bytes_since_meta` and `last_meta_str` across calls so metadata
/// boundaries stay aligned and change detection works across chunks.
pub(crate) fn process_audio_with_icy(
    chunk: Bytes,
    metaint: usize,
    bytes_since_meta: &mut usize,
    last_meta_str: &mut String,
    current_meta_str: &str,
    stream_url: Option<&str>,
) -> Vec<Bytes> {
    let mut output = Vec::new();
    let mut offset = 0;
    while offset < chunk.len() {
        let remaining_in_interval = metaint - *bytes_since_meta;
        let consume = remaining_in_interval.min(chunk.len() - offset);

        if consume > 0 {
            output.push(chunk.slice(offset..offset + consume));
            offset += consume;
            *bytes_since_meta += consume;
        }

        if *bytes_since_meta >= metaint {
            let block: Vec<u8> = if *current_meta_str != *last_meta_str {
                *last_meta_str = current_meta_str.to_string();
                build_icy_metadata_block(last_meta_str, stream_url)
            } else {
                vec![0x00]
            };
            output.push(Bytes::from(block));
            *bytes_since_meta = 0;
        }
    }
    output
}

/// Render the info page HTML by substituting placeholders in the template via string replacement.
pub(crate) fn render_info_page(ctx: &InfoPageContext, template: &str) -> String {
    let mut html = template.to_owned();

    // Process {% for stream in streams %}...{% endfor %} blocks
    while let Some(start) = html.find("{% for stream in streams %}") {
        let body_start = start + "{% for stream in streams %}".len();
        let end = html[body_start..]
            .find("{% endfor %}")
            .map(|p| body_start + p)
            .expect("missing {% endfor %} in template");
        let inner = html[body_start..end].to_owned();

        let mut replacement = String::new();
        for stream in &ctx.streams {
            replacement.push_str(
                &inner
                    .replace("{{ stream.name }}", &stream.name)
                    .replace("{{ stream.bitrate }}", &stream.bitrate.to_string())
                    .replace("{{ stream.url }}", &stream.url),
            );
        }

        let endfor_end = end + "{% endfor %}".len();
        html.replace_range(start..endfor_end, &replacement);
    }

    html.replace("{{ station_name }}", &ctx.station_name)
        .replace("{{ first_stream }}", &ctx.first_stream)
        .replace("{{ station_genre }}", &ctx.station_genre)
        .replace("{{ station_description }}", &ctx.station_description)
        .replace("{{ current_track }}", &ctx.current_track)
        .replace("{{ album }}", &ctx.album)
        .replace("{{ bitrate }}", &ctx.bitrate.to_string())
        .replace("{{ public_url }}", &ctx.public_url)
        .replace("{{ cover_url }}", &ctx.cover_url)
        .replace("{{ version }}", &ctx.version)
        .replace("{{ listeners }}", &ctx.listeners.to_string())
        .replace("{{ uptime }}", &ctx.uptime)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- build_icy_metadata_block tests ---

    #[test]
    fn given_short_title_when_building_metadata_block_then_pads_to_16_bytes() {
        // "Artist - Title" = 15 chars
        // "StreamTitle='Artist - Title';StreamUrl='';" = 13 + 15 + 18 = 46 chars
        // N = ceil(46 / 16) = 3, padded = 48 bytes
        let block = build_icy_metadata_block("Artist - Title", None);
        assert_eq!(block.len(), 1 + 3 * 16); // 49 bytes
        assert_eq!(block[0], 3); // 3 blocks
        let expected_prefix = b"StreamTitle='Artist - Title';StreamUrl='';";
        assert_eq!(&block[1..1 + expected_prefix.len()], expected_prefix);
        // Remaining bytes should be null padding
        assert!(block[1 + expected_prefix.len()..].iter().all(|&b| b == 0));
    }

    #[test]
    fn given_exact_16_boundary_when_building_metadata_block_then_correct_block_count() {
        // 31-byte payload → ceil(31/16) = 2 blocks
        let block = build_icy_metadata_block("AB", None);
        assert_eq!(block[0], 2);
        assert_eq!(block.len(), 1 + 2 * 16);
    }

    #[test]
    fn given_empty_title_when_building_metadata_block_then_still_valid() {
        let block = build_icy_metadata_block("", None);
        assert_eq!(block[0], 2); // "StreamTitle='';StreamUrl='';" = 30 chars → ceil(30/16)=2
        assert!(block[1..].iter().any(|&b| b != 0)); // has content
    }

    #[test]
    fn given_very_long_title_when_building_metadata_block_then_clamped_to_255_blocks() {
        let long = "X".repeat(5000);
        let block = build_icy_metadata_block(&long, None);
        assert_eq!(block[0], 255); // clamped
        assert_eq!(block.len(), 1 + 255 * 16); // 4081 bytes
    }

    #[test]
    fn given_special_characters_when_building_metadata_block_then_utf8_preserved() {
        let block = build_icy_metadata_block("Motörhead - Åce of Spädes", None);
        let content = String::from_utf8_lossy(&block[1..]);
        assert!(content.contains("Motörhead"));
        assert!(content.contains("Åce of Spädes"));
    }

    #[test]
    fn given_stream_url_when_building_metadata_block_then_url_included() {
        let block =
            build_icy_metadata_block("Artist - Title", Some("http://example.com/cover.jpg"));
        let content = String::from_utf8_lossy(&block[1..]);
        assert!(content.contains("StreamTitle='Artist - Title';"));
        assert!(content.contains("StreamUrl='http://example.com/cover.jpg';"));
    }

    // --- process_audio_with_icy tests ---

    #[test]
    fn given_bytes_since_meta_zero_when_short_chunk_then_all_output_is_audio() {
        // Regression: no metadata block must appear at byte 0 of the stream.
        let chunk = Bytes::from(vec![0xAA; 100]);
        let mut bytes_since_meta = 0;
        let mut last_meta_str = String::new();

        let blocks = process_audio_with_icy(
            chunk,
            16000,
            &mut bytes_since_meta,
            &mut last_meta_str,
            "Some Artist - Some Track",
            None,
        );

        // All blocks are audio — no metadata injected
        assert!(!blocks.is_empty());
        for block in &blocks {
            assert!(
                block.len() > 1 || !block.is_empty(),
                "all blocks under metaint should be audio chunks"
            );
        }
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].len(), 100);
        assert_eq!(bytes_since_meta, 100);
        assert_eq!(last_meta_str, ""); // unchanged — no boundary hit
    }

    #[test]
    fn given_bytes_near_metaint_when_chunk_crosses_boundary_then_metadata_injected() {
        // bytes_since_meta at 15900, chunk of 200 bytes.
        // Should produce: 100 bytes audio, metadata block, 100 bytes audio.
        let chunk = Bytes::from(vec![0xBB; 200]);
        let mut bytes_since_meta = 15900;
        let mut last_meta_str = String::new();

        let blocks = process_audio_with_icy(
            chunk,
            16000,
            &mut bytes_since_meta,
            &mut last_meta_str,
            "Artist - Title",
            None,
        );

        assert_eq!(blocks.len(), 3, "audio, metadata, audio");
        // First block: audio, 100 bytes (fills remaining interval)
        assert_eq!(blocks[0].len(), 100);
        // Second block: metadata block
        assert!(
            blocks[1].len() > 1,
            "metadata block must be more than empty \\x00"
        );
        // Third block: remaining 100 bytes of audio
        assert_eq!(blocks[2].len(), 100);
        // Counter must reset after metadata boundary
        assert_eq!(bytes_since_meta, 100);
        // last_meta_str must be updated
        assert_eq!(last_meta_str, "Artist - Title");
    }

    #[test]
    fn given_metadata_unchanged_when_boundary_hit_then_empty_block() {
        let chunk = Bytes::from(vec![0xCC; 16000]);
        let mut bytes_since_meta = 15900;
        let mut last_meta_str = String::from("Artist - Title");

        let blocks = process_audio_with_icy(
            chunk,
            16000,
            &mut bytes_since_meta,
            &mut last_meta_str,
            "Artist - Title", // same as last_meta_str
            None,
        );

        assert_eq!(blocks.len(), 3);
        // Metadata block should be single zero byte
        assert_eq!(blocks[1].len(), 1);
        assert_eq!(blocks[1][0], 0x00);
        // last_meta_str unchanged
        assert_eq!(last_meta_str, "Artist - Title");
    }

    #[test]
    fn given_metadata_changed_when_boundary_hit_then_full_block_and_counter_update() {
        let chunk = Bytes::from(vec![0xDD; 16000]);
        let mut bytes_since_meta = 15900;
        let mut last_meta_str = String::from("Old Artist - Old Title");

        let blocks = process_audio_with_icy(
            chunk,
            16000,
            &mut bytes_since_meta,
            &mut last_meta_str,
            "New Artist - New Title",
            None,
        );

        assert_eq!(blocks.len(), 3);
        // Metadata block must be > 1 byte (not empty)
        assert!(blocks[1].len() > 1);
        // last_meta_str updates to new value
        assert_eq!(last_meta_str, "New Artist - New Title");
    }

    #[test]
    fn given_chunk_spanning_multiple_boundaries_then_metadata_at_each() {
        // 50000 bytes, metaint=16000 → boundaries at 16000, 32000, 48000
        // Metadata unchanged → all empty blocks
        let chunk = Bytes::from(vec![0xEE; 50000]);
        let mut bytes_since_meta = 0;
        let mut last_meta_str = String::from("Track A - Song A");

        let blocks = process_audio_with_icy(
            chunk,
            16000,
            &mut bytes_since_meta,
            &mut last_meta_str,
            "Track A - Song A", // unchanged
            None,
        );

        // Layout: A=[16000] M=[empty] A=[16000] M=[empty] A=[16000] M=[empty] A=[2000]
        assert_eq!(blocks.len(), 7);
        assert_eq!(blocks[0].len(), 16000);
        assert_eq!(blocks[1].len(), 1);
        assert_eq!(blocks[1][0], 0x00);
        assert_eq!(blocks[2].len(), 16000);
        assert_eq!(blocks[3].len(), 1);
        assert_eq!(blocks[3][0], 0x00);
        assert_eq!(blocks[4].len(), 16000);
        assert_eq!(blocks[5].len(), 1);
        assert_eq!(blocks[5][0], 0x00);
        assert_eq!(blocks[6].len(), 2000);
        assert_eq!(bytes_since_meta, 2000);
        assert_eq!(last_meta_str, "Track A - Song A");
    }

    #[test]
    fn given_chunk_exactly_at_metaint_boundary_then_only_metadata_emitted() {
        // bytes_since_meta already at metaint, so chunk should trigger metadata right away
        let chunk = Bytes::from(vec![0xFF; 100]);
        let mut bytes_since_meta = 16000;
        let mut last_meta_str = String::new();

        let blocks = process_audio_with_icy(
            chunk,
            16000,
            &mut bytes_since_meta,
            &mut last_meta_str,
            "Artist - Track",
            None,
        );

        // First: metadata (boundary already reached), then 100 bytes audio
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].len() > 1, "first block is metadata");
        assert_eq!(blocks[1].len(), 100, "second block is audio");
        assert_eq!(bytes_since_meta, 100);
    }

    #[test]
    fn given_empty_chunk_when_processing_then_no_output() {
        let chunk = Bytes::new();
        let mut bytes_since_meta = 0;
        let mut last_meta_str = String::new();

        let blocks = process_audio_with_icy(
            chunk,
            16000,
            &mut bytes_since_meta,
            &mut last_meta_str,
            "Artist - Track",
            None,
        );

        assert!(blocks.is_empty());
        assert_eq!(bytes_since_meta, 0);
        assert_eq!(last_meta_str, "");
    }

    // --- extract_cover_from_file tests ---

    #[test]
    fn given_file_without_cover_when_extracting_cover_then_returns_none() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        // Write minimal valid MP3 without ID3 tags (or with empty tags)
        // audiotags will fail to find tags, so album() returns None
        tmp.write_all(b"\xFF\xFB\x90\x00").unwrap();
        let result = extract_cover_from_file(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn given_file_with_cover_when_extracting_cover_then_returns_image() {
        // Minimal 1x1 white JPEG
        let jpeg_data: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00,
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06,
            0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09, 0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D,
            0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12, 0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D,
            0x1A, 0x1C, 0x1C, 0x20, 0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28,
            0x37, 0x29, 0x2C, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32,
            0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01,
            0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01,
            0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
            0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x10,
            0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00,
            0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06,
            0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42,
            0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16,
            0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37,
            0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55,
            0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73,
            0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
            0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5,
            0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA,
            0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6,
            0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA,
            0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x0C,
            0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xD9,
        ];

        // Build a valid MP3 file with an ID3v2 tag containing cover art,
        // using the `id3` crate (already a transitive dependency via audiotags).
        use id3::frame::{Content, Picture, PictureType};
        use id3::{Frame, TagLike};

        let mut tag = id3::Tag::new();
        let picture = Picture {
            mime_type: "image/jpeg".to_string(),
            picture_type: PictureType::CoverFront,
            description: "cover".to_string(),
            data: jpeg_data.to_vec(),
        };
        tag.add_frame(Frame::with_content("APIC", Content::Picture(picture)));
        // audiotags requires a TALB frame for album() to return Some
        tag.set_album("Test Album");

        let mut mp3_bytes = Vec::new();
        tag.write_to(&mut mp3_bytes, id3::Version::Id3v23)
            .expect("failed to write ID3 tag");
        mp3_bytes.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        mp3_bytes.resize(mp3_bytes.len() + 100, 0x00);
        // audiotags detects format by file extension, so use .mp3
        let tmpdir = tempfile::tempdir().unwrap();
        let audio_path = tmpdir.path().join("test.mp3");
        std::fs::write(&audio_path, &mp3_bytes).unwrap();

        let result = extract_cover_from_file(&audio_path);
        assert!(result.is_some(), "expected cover art to be extracted");
        let (_data, mime_type) = result.unwrap();
        assert_eq!(mime_type, "image/jpeg");
    }

    // --- process_audio_with_icy StreamUrl test ---

    #[test]
    fn given_stream_url_when_processing_audio_then_metadata_block_contains_stream_url() {
        let chunk = Bytes::from(vec![0u8; 32000]); // spans two 16000 boundaries
        let mut bytes_since_meta: usize = 0;
        let mut last_meta_str = String::new();
        let stream_url = Some("http://example.com/cover.jpg");

        let blocks = process_audio_with_icy(
            chunk,
            16000,
            &mut bytes_since_meta,
            &mut last_meta_str,
            "Artist - Title",
            stream_url,
        );

        // We should have at least one metadata block
        assert!(!blocks.is_empty());
        // Find the metadata block (the one starting with a non-zero count byte)
        let meta_block = blocks
            .iter()
            .find(|b| b.len() > 1 && b[0] > 0)
            .expect("expected a metadata block");
        let content = String::from_utf8_lossy(&meta_block[1..]);
        assert!(
            content.contains("StreamUrl='http://example.com/cover.jpg'"),
            "expected StreamUrl in metadata, got: {}",
            content
        );
    }

    // --- render_info_page tests ---

    #[cfg(test)]
    fn info_page_context_with_streams(streams: Vec<StreamLink>) -> InfoPageContext {
        InfoPageContext {
            station_name: "Funkstrom FM".to_string(),
            current_track: "Artist - Track".to_string(),
            album: "The Album".to_string(),
            station_description: "A radio station".to_string(),
            station_genre: "Electronic".to_string(),
            bitrate: 320,
            public_url: "https://radio.example.com".to_string(),
            streams,
            first_stream: "/stream".to_string(),
            cover_url: "https://radio.example.com/cover.jpg".to_string(),
            version: "1.2.3".to_string(),
            listeners: 42,
            uptime: "1h 2m".to_string(),
        }
    }

    #[test]
    fn given_template_with_loop_when_two_streams_then_body_repeated_per_stream() {
        let ctx = info_page_context_with_streams(vec![
            StreamLink {
                name: "lo".to_string(),
                bitrate: 128,
                url: "/low".to_string(),
            },
            StreamLink {
                name: "hi".to_string(),
                bitrate: 320,
                url: "/high".to_string(),
            },
        ]);
        let template = "A{% for stream in streams %}({{ stream.name }}|{{ stream.bitrate }}|{{ stream.url }}){% endfor %}B";

        let rendered = render_info_page(&ctx, template);

        assert_eq!(rendered, "A(lo|128|/low)(hi|320|/high)B");
    }

    #[test]
    fn given_template_with_loop_when_no_streams_then_loop_block_removed_entirely() {
        let ctx = info_page_context_with_streams(Vec::new());
        let template = "A{% for stream in streams %}({{ stream.name }}){% endfor %}B";

        let rendered = render_info_page(&ctx, template);

        assert_eq!(rendered, "AB");
    }

    #[test]
    fn given_stream_placeholders_in_loop_when_single_stream_then_all_substituted() {
        let ctx = info_page_context_with_streams(vec![StreamLink {
            name: "main".to_string(),
            bitrate: 256,
            url: "https://radio.example.com/main".to_string(),
        }]);
        let template =
            "{% for stream in streams %}<a href=\"{{ stream.url }}\">{{ stream.name }} {{ stream.bitrate }}</a>{% endfor %}";

        let rendered = render_info_page(&ctx, template);

        assert_eq!(
            rendered,
            "<a href=\"https://radio.example.com/main\">main 256</a>"
        );
    }

    #[test]
    fn given_scalar_placeholders_when_rendering_then_each_substituted_with_context_value() {
        let ctx = info_page_context_with_streams(Vec::new());
        let template =
            "{{ station_name }}|{{ first_stream }}|{{ station_genre }}|{{ station_description }}\
                        |{{ current_track }}|{{ album }}|{{ bitrate }}|{{ public_url }}|{{ cover_url }}\
                        |{{ version }}|{{ listeners }}|{{ uptime }}";

        let rendered = render_info_page(&ctx, template);

        assert_eq!(
            rendered,
            "Funkstrom FM|/stream|Electronic|A radio station|Artist - Track|The Album|320|https://radio.example.com\
             |https://radio.example.com/cover.jpg|1.2.3|42|1h 2m"
        );
    }
}
