//! What a device plays, as the server is told to pick a file by it. A shell reports what its decoders and screen can
//! do (raw facts); the core names it, so the phone and the PC speak the same words.
//!
//! The phone and TV list only their hardware decoders (a weak box decoding HEVC or 10-bit in software stutters; the
//! shell finds the hardware); a PC's mpv decodes everything, so it claims everything ([`everything`]). That's on purpose.

/// What a device's hardware decoders and screen can do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decoders {
    /// A hardware HEVC (H.265) decoder.
    pub hevc: bool,
    /// ... one that plays Main 10 (10-bit).
    pub hevc_main10: bool,
    /// A hardware AV1 decoder.
    pub av1: bool,
    /// The screen shows HDR.
    pub hdr_screen: bool,
}

/// The codec words a device claims, and whether it claims HDR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    pub codecs: Vec<String>,
    pub hdr: bool,
}

/// The sound formats every device claims (the phone's FFmpeg decoder covers what its hardware can't).
const SOUND: [&str; 7] = ["ac3", "eac3", "dts", "truehd", "pcm", "opus", "flac"];

/// The words for a device with these decoders. HDR is claimed when the screen shows it and 10-bit HEVC plays.
pub fn claim(d: Decoders) -> Claimed {
    let mut codecs = vec!["h264".to_string()];
    for (on, word) in [(d.hevc, "hevc"), (d.hevc_main10, "hevc10"), (d.av1, "av1")] {
        if on {
            codecs.push(word.into());
        }
    }
    codecs.extend(SOUND.iter().map(|s| s.to_string()));
    let hdr = d.hdr_screen && d.hevc_main10;
    if hdr {
        codecs.push("hdr".into());
    }
    Claimed { codecs, hdr }
}

/// Everything, for a PC: mpv decodes what the graphics card can't in software. (`hdr10` and the two ways a file
/// can be local are claimed too.)
pub fn everything() -> Claimed {
    let codecs = [
        "h264", "hevc", "hevc10", "av1", "ac3", "eac3", "dts", "truehd", "pcm", "opus", "flac", "hdr", "hdr10", "local", "file",
    ];
    Claimed { codecs: codecs.iter().map(|c| c.to_string()).collect(), hdr: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_names_what_its_hardware_plays() {
        let weak = claim(Decoders::default());
        assert_eq!(weak.codecs[0], "h264");
        assert!(!weak.codecs.contains(&"hevc".to_string()) && weak.codecs.contains(&"flac".to_string()) && !weak.hdr);
        let full = claim(Decoders { hevc: true, hevc_main10: true, av1: true, hdr_screen: true });
        assert_eq!(&full.codecs[..4], ["h264", "hevc", "hevc10", "av1"]);
        assert_eq!(full.codecs.last().map(String::as_str), Some("hdr"));
        assert!(full.hdr);
    }

    #[test]
    fn hdr_needs_the_screen_and_ten_bit() {
        assert!(!claim(Decoders { hevc: true, hevc_main10: true, ..Decoders::default() }).hdr);
        assert!(!claim(Decoders { hevc: true, hdr_screen: true, ..Decoders::default() }).hdr);
        assert!(everything().hdr && everything().codecs.contains(&"local".to_string()));
    }
}
