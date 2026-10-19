//! The sources Edith can ask, and what each shows and asks for.

use crate::types::Kind;

/// The sources Edith can ask, in the order they are listed.
pub const SOURCES: [&str; 7] = ["pexels", "pixabay", "wikimedia", "archive", "freesound", "openverse", "nasa"];

pub struct SourceInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub kinds: &'static [Kind],
    /// What the site asks to be shown next to its results.
    pub credit: &'static str,
    pub credit_url: &'static str,
    /// Needs a key from the person (the others are open).
    pub needs_key: bool,
}

pub fn info(id: &str) -> Option<SourceInfo> {
    Some(match id {
        "pexels" => SourceInfo {
            id: "pexels",
            label: "Pexels",
            kinds: &[Kind::Video, Kind::Image],
            credit: "Photos and videos provided by Pexels",
            credit_url: "https://www.pexels.com",
            needs_key: true,
        },
        "pixabay" => SourceInfo {
            id: "pixabay",
            label: "Pixabay",
            kinds: &[Kind::Video, Kind::Image],
            credit: "Images and videos from Pixabay",
            credit_url: "https://pixabay.com",
            needs_key: true,
        },
        "freesound" => SourceInfo {
            id: "freesound",
            label: "Freesound",
            kinds: &[Kind::Audio],
            credit: "Sounds from Freesound",
            credit_url: "https://freesound.org",
            needs_key: true,
        },
        "wikimedia" => SourceInfo {
            id: "wikimedia",
            label: "Wikimedia Commons",
            kinds: &[Kind::Video, Kind::Image, Kind::Audio],
            credit: "Media from Wikimedia Commons",
            credit_url: "https://commons.wikimedia.org",
            needs_key: false,
        },
        "archive" => SourceInfo {
            id: "archive",
            label: "Internet Archive",
            kinds: &[Kind::Video, Kind::Image, Kind::Audio],
            credit: "Media from the Internet Archive",
            credit_url: "https://archive.org",
            needs_key: false,
        },
        "openverse" => SourceInfo {
            id: "openverse",
            label: "Openverse",
            kinds: &[Kind::Image, Kind::Audio],
            credit: "Media found with Openverse",
            credit_url: "https://openverse.org",
            needs_key: false,
        },
        "nasa" => SourceInfo {
            id: "nasa",
            label: "NASA Image and Video Library",
            kinds: &[Kind::Video, Kind::Image],
            credit: "Media from the NASA Image and Video Library",
            credit_url: "https://images.nasa.gov",
            needs_key: false,
        },
        _ => return None,
    })
}
