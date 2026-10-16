//! The one request shape and the one candidate shape every source translates to and from.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Video,
    Image,
    Audio,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Video, Kind::Image, Kind::Audio];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Video => "video",
            Kind::Image => "image",
            Kind::Audio => "audio",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Landscape,
    Portrait,
    Square,
}

impl Orientation {
    pub fn parse(s: &str) -> Option<Orientation> {
        match s {
            "landscape" => Some(Orientation::Landscape),
            "portrait" => Some(Orientation::Portrait),
            "square" => Some(Orientation::Square),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Orientation::Landscape => "landscape",
            Orientation::Portrait => "portrait",
            Orientation::Square => "square",
        }
    }
}

/// What the person is looking for. The same request goes to every source; each translates it to
/// what its site understands and ignores what it cannot use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Request {
    /// Plain words: "a lone figure walking through fog at dawn", or keywords.
    pub text: String,
    pub kinds: Vec<Kind>,
    pub orientation: Option<Orientation>,
    /// The smallest picture wanted, in pixels (video and images).
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    /// Shortest and longest length in seconds (video and audio).
    pub min_seconds: Option<f64>,
    pub max_seconds: Option<f64>,
    /// Results wanted from each source (1 to 40).
    pub count: u32,
    /// 1-based page, for "more".
    pub page: u32,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            text: String::new(),
            kinds: vec![Kind::Video],
            orientation: None,
            min_width: None,
            min_height: None,
            min_seconds: None,
            max_seconds: None,
            count: 12,
            page: 1,
        }
    }
}

impl Request {
    pub fn wants(&self, k: Kind) -> bool {
        self.kinds.contains(&k)
    }

    /// This request with every number forced into range, and the kinds listed once.
    pub fn sanitized(mut self) -> Self {
        self.text = self.text.chars().filter(|c| !c.is_control()).take(300).collect::<String>().trim().to_string();
        self.count = self.count.clamp(1, 40);
        self.page = self.page.clamp(1, 50);
        let mut seen = Vec::new();
        for k in self.kinds.drain(..) {
            if !seen.contains(&k) {
                seen.push(k);
            }
        }
        self.kinds = if seen.is_empty() { vec![Kind::Video] } else { seen };
        let ok = |v: Option<f64>| v.filter(|x| x.is_finite() && *x >= 0.0 && *x <= 86_400.0);
        self.min_seconds = ok(self.min_seconds);
        self.max_seconds = ok(self.max_seconds);
        self.min_width = self.min_width.map(|w| w.min(16_000));
        self.min_height = self.min_height.map(|h| h.min(16_000));
        self
    }
}

/// What a site says you may do with a file, as it says it. `None` means "not stated".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Rights {
    pub name: String,
    pub url: String,
    pub commercial: Option<bool>,
    pub attribution_required: Option<bool>,
}

/// Where a candidate can be fetched from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub url: String,
    /// The file extension to save it under (`mp4`, `jpg`, `mp3`).
    pub ext: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: Option<u64>,
    /// What this file is when it is not the original (`preview quality`), shown to the person.
    pub note: String,
    /// `url` is not the file but a description of the item (Internet Archive): the best file is
    /// looked up and chosen when the person adds it.
    #[serde(default)]
    pub resolve: bool,
}

/// One result: enough to show it, decide on it, and (once chosen) fetch it and record where it
/// came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// `source:native id`, unique across sources.
    pub id: String,
    pub source: String,
    pub kind: Kind,
    pub title: String,
    pub author: String,
    pub author_url: String,
    /// The page the file lives on.
    pub source_url: String,
    /// A small picture to show in the grid (empty when the source has none).
    pub thumb: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub seconds: Option<f64>,
    pub tags: Vec<String>,
    pub rights: Rights,
    pub download: Option<Download>,
}

/// One page of results from one source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub candidates: Vec<Candidate>,
    /// Matches the site reports (may be larger than what is returned).
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SourceError {
    #[error(transparent)]
    Net(#[from] edith_net::NetError),
    /// No key stored, or the site refused it.
    #[error("{0}")]
    Auth(String),
    #[error("{0} is limiting requests; try again in a moment")]
    RateLimited(String),
    #[error("{0}")]
    BadReply(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("stopped")]
    Cancelled,
}

impl SourceError {
    pub(crate) fn bad(what: &str) -> Self {
        SourceError::BadReply(edith_net::redact::message(what))
    }
}
