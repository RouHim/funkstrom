use log::warn;

// Config validation (config.rs StreamConfig::validate) only accepts mp3/aac/opus/ogg.
// The vorbis and flac arms below are defensive for direct/internal callers and are
// intentionally kept in sync with get_muxer_for_format.
pub(crate) fn get_codec_for_format(format: &str) -> &str {
    match format {
        "mp3" => "libmp3lame",
        "opus" => "libopus",
        "aac" => "aac",
        "vorbis" | "ogg" => "libvorbis",
        "flac" => "flac",
        _ => {
            warn!("Unknown format '{}', defaulting to libmp3lame", format);
            "libmp3lame"
        }
    }
}

pub(crate) fn get_muxer_for_format(format: &str) -> &str {
    match format {
        "aac" => "adts",
        "mp3" => "mp3",
        "opus" => "opus",
        "vorbis" | "ogg" => "ogg",
        "flac" => "flac",
        _ => {
            warn!("Unknown format '{}', defaulting to mp3 muxer", format);
            "mp3"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_mp3_format_when_getting_codec_then_returns_libmp3lame() {
        assert_eq!(get_codec_for_format("mp3"), "libmp3lame");
    }

    #[test]
    fn given_opus_format_when_getting_codec_then_returns_libopus() {
        assert_eq!(get_codec_for_format("opus"), "libopus");
    }

    #[test]
    fn given_aac_format_when_getting_codec_then_returns_aac() {
        assert_eq!(get_codec_for_format("aac"), "aac");
    }

    #[test]
    fn given_vorbis_format_when_getting_codec_then_returns_libvorbis() {
        assert_eq!(get_codec_for_format("vorbis"), "libvorbis");
    }

    #[test]
    fn given_ogg_format_when_getting_codec_then_returns_libvorbis() {
        assert_eq!(get_codec_for_format("ogg"), "libvorbis");
    }

    #[test]
    fn given_flac_format_when_getting_codec_then_returns_flac() {
        assert_eq!(get_codec_for_format("flac"), "flac");
    }

    #[test]
    fn given_unknown_format_when_getting_codec_then_returns_default_libmp3lame() {
        assert_eq!(get_codec_for_format("unknown"), "libmp3lame");
    }

    #[test]
    fn given_aac_format_when_getting_muxer_then_returns_adts() {
        assert_eq!(get_muxer_for_format("aac"), "adts");
    }

    #[test]
    fn given_mp3_format_when_getting_muxer_then_returns_mp3() {
        assert_eq!(get_muxer_for_format("mp3"), "mp3");
    }

    #[test]
    fn given_opus_format_when_getting_muxer_then_returns_opus() {
        assert_eq!(get_muxer_for_format("opus"), "opus");
    }

    #[test]
    fn given_vorbis_format_when_getting_muxer_then_returns_ogg() {
        assert_eq!(get_muxer_for_format("vorbis"), "ogg");
    }

    #[test]
    fn given_ogg_format_when_getting_muxer_then_returns_ogg() {
        assert_eq!(get_muxer_for_format("ogg"), "ogg");
    }

    #[test]
    fn given_flac_format_when_getting_muxer_then_returns_flac() {
        assert_eq!(get_muxer_for_format("flac"), "flac");
    }

    #[test]
    fn given_unknown_format_when_getting_muxer_then_returns_default_mp3() {
        assert_eq!(get_muxer_for_format("unknown"), "mp3");
    }
}
