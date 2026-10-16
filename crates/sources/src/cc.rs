//! Reading a licence as a site writes it (a name, or a Creative Commons address) into what it
//! allows. Shared by every source that carries licences per file: Freesound, Wikimedia Commons and
//! the Internet Archive. What is not understood claims nothing, so the audit calls it a risk.

use crate::types::Rights;

fn cut(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(80).collect::<String>().trim().to_string()
}

/// The part of a Creative Commons address that names the licence: `by-nc-sa` from
/// `.../licenses/by-nc-sa/4.0/`, and its version.
fn parts(url: &str) -> (String, String) {
    let after = url.split("/licenses/").nth(1).or_else(|| url.split("/publicdomain/").nth(1)).unwrap_or("");
    let mut it = after.split('/').filter(|p| !p.is_empty());
    let kind = it.next().unwrap_or("").to_ascii_lowercase();
    let version = it.next().filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit())).unwrap_or("").to_string();
    (kind, version)
}

/// What `license` (a name such as `Attribution NonCommercial` or `CC BY-SA 4.0`, or an address)
/// allows.
pub fn rights(license: &str) -> Rights {
    let raw = license.trim();
    let l = raw.to_ascii_lowercase();
    let url = if l.starts_with("http") { raw.chars().filter(|c| !c.is_control() && !c.is_whitespace()).take(300).collect() } else { String::new() };
    let (kind, version) = parts(&url);
    let free = |name: &str| Rights { name: name.into(), url: url.clone(), commercial: Some(true), attribution_required: Some(false) };
    let cc = |name: String, commercial: bool| Rights { name, url: url.clone(), commercial: Some(commercial), attribution_required: Some(true) };
    let versioned = |base: &str| if version.is_empty() { base.to_string() } else { format!("{base} {version}") };

    if l.contains("publicdomain/zero") || l.contains("/zero/") || l == "creative commons 0" || l == "cc0" || l.starts_with("cc0") {
        return free("CC0 1.0");
    }
    if l.contains("publicdomain/mark") || l == "public domain mark" {
        return free("Public Domain Mark 1.0");
    }
    if l.contains("licenses/publicdomain") || l == "public domain" || l == "pd" || l.starts_with("public domain") {
        return free("Public domain");
    }
    // by, by-sa, by-nd, by-nc, by-nc-sa, by-nc-nd: from the address, else from the words
    let tag = if kind.starts_with("by") {
        kind
    } else if l.contains("noncommercial") || l.contains("non-commercial") || l.contains("by-nc") || l.contains("by nc") {
        if l.contains("sharealike") || l.contains("by-nc-sa") || l.contains("nc-sa") { "by-nc-sa".into() } else { "by-nc".into() }
    } else if l.contains("by-sa") || l.contains("by sa") || l.contains("sharealike") {
        "by-sa".into()
    } else if l.contains("by-nd") || l.contains("noderiv") {
        "by-nd".into()
    } else if l == "attribution" || l == "cc by" || l.starts_with("cc by ") || l.starts_with("cc-by") {
        "by".into()
    } else {
        String::new()
    };
    if tag.is_empty() {
        return Rights { name: cut(raw), url, commercial: None, attribution_required: None };
    }
    let commercial = !tag.contains("nc");
    // keep a version the name itself carries (`CC BY-SA 4.0`) when the address gave none
    let named_version = raw.rsplit(' ').next().filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()) && v.contains('.')).unwrap_or("");
    let base = format!("CC {}", tag.to_ascii_uppercase());
    let name = if version.is_empty() && !named_version.is_empty() { format!("{base} {named_version}") } else { versioned(&base) };
    cc(name, commercial)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> (String, Option<bool>, Option<bool>) {
        let r = rights(s);
        (r.name, r.commercial, r.attribution_required)
    }

    #[test]
    fn public_domain_in_all_its_spellings_is_free() {
        assert_eq!(r("http://creativecommons.org/publicdomain/zero/1.0/"), ("CC0 1.0".into(), Some(true), Some(false)));
        assert_eq!(r("Creative Commons 0"), ("CC0 1.0".into(), Some(true), Some(false)));
        assert_eq!(r("CC0"), ("CC0 1.0".into(), Some(true), Some(false)));
        assert_eq!(r("http://creativecommons.org/publicdomain/mark/1.0/"), ("Public Domain Mark 1.0".into(), Some(true), Some(false)));
        assert_eq!(r("http://creativecommons.org/licenses/publicdomain/"), ("Public domain".into(), Some(true), Some(false)));
        assert_eq!(r("Public domain"), ("Public domain".into(), Some(true), Some(false)));
    }

    #[test]
    fn attribution_licences_need_a_credit_and_noncommercial_ones_are_not_for_commercial_use() {
        assert_eq!(r("https://creativecommons.org/licenses/by/4.0/"), ("CC BY 4.0".into(), Some(true), Some(true)));
        assert_eq!(r("http://creativecommons.org/licenses/by-sa/3.0/"), ("CC BY-SA 3.0".into(), Some(true), Some(true)));
        assert_eq!(r("https://creativecommons.org/licenses/by-nd/2.0/"), ("CC BY-ND 2.0".into(), Some(true), Some(true)));
        assert_eq!(r("http://creativecommons.org/licenses/by-nc/3.0/"), ("CC BY-NC 3.0".into(), Some(false), Some(true)));
        assert_eq!(r("https://creativecommons.org/licenses/by-nc-sa/4.0/"), ("CC BY-NC-SA 4.0".into(), Some(false), Some(true)));
        assert_eq!(r("Attribution"), ("CC BY".into(), Some(true), Some(true)));
        assert_eq!(r("Attribution NonCommercial"), ("CC BY-NC".into(), Some(false), Some(true)));
        assert_eq!(r("CC BY-SA 4.0"), ("CC BY-SA 4.0".into(), Some(true), Some(true)));
        assert_eq!(r("CC BY 2.5"), ("CC BY 2.5".into(), Some(true), Some(true)));
    }

    #[test]
    fn what_is_not_understood_claims_nothing() {
        assert_eq!(r("Sampling+"), ("Sampling+".into(), None, None));
        assert_eq!(r("GFDL"), ("GFDL".into(), None, None));
        assert_eq!(r(""), (String::new(), None, None));
        assert_eq!(r("https://example.test/terms").1, None);
        assert!(rights("x\u{7}y\u{0}z".repeat(200).as_str()).name.chars().count() <= 80);
    }

    #[test]
    fn the_address_is_kept_but_only_if_it_is_one() {
        assert_eq!(rights("https://creativecommons.org/licenses/by/4.0/").url, "https://creativecommons.org/licenses/by/4.0/");
        assert_eq!(rights("Attribution").url, "");
        assert!(rights("http://x/ y").url.is_empty() || !rights("http://x/ y").url.contains(' '));
    }
}
