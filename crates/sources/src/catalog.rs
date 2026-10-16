//! The sources Edith can ask, and what each shows and asks for.

use crate::types::Kind;

/// The sources Edith can ask, in the order they are listed.
pub const SOURCES: [&str; 3] = ["pexels", "pixabay", "freesound"];

pub struct SourceInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub kinds: &'static [Kind],
    /// What the site asks to be shown next to its results.
    pub credit: &'static str,
    pub credit_url: &'static str,
}

pub fn info(id: &str) -> Option<SourceInfo> {
    Some(match id {
        "pexels" => SourceInfo {
            id: "pexels",
            label: "Pexels",
            kinds: &[Kind::Video, Kind::Image],
            credit: "Photos and videos provided by Pexels",
            credit_url: "https://www.pexels.com",
        },
        "pixabay" => SourceInfo {
            id: "pixabay",
            label: "Pixabay",
            kinds: &[Kind::Video, Kind::Image],
            credit: "Images and videos from Pixabay",
            credit_url: "https://pixabay.com",
        },
        "freesound" => {
            SourceInfo { id: "freesound", label: "Freesound", kinds: &[Kind::Audio], credit: "Sounds from Freesound", credit_url: "https://freesound.org" }
        }
        _ => return None,
    })
}
