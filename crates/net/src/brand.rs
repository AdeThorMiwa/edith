//! The product name as the fork writes it. Everything the fork creates that shows a name to a
//! person or a server reads it from here, so the rename to Edith is one edit.

/// The product name.
pub const NAME: &str = "Edith";

/// The keychain "service" every secret is filed under.
pub const KEYCHAIN_SERVICE: &str = "edith";

/// Where the project lives (sent in the `User-Agent` so a provider can find out who is calling).
pub const HOME_URL: &str = "https://github.com/AdeThorMiwa/filmcraft";

/// `Edith/<version> (video editor; <url>)`.
pub fn user_agent() -> String {
    format!("{NAME}/{} (video editor; {HOME_URL})", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_agent_names_the_product_and_the_version() {
        let ua = user_agent();
        assert!(ua.starts_with("Edith/") && ua.contains(env!("CARGO_PKG_VERSION")) && ua.contains(HOME_URL), "{ua}");
        assert!(KEYCHAIN_SERVICE.chars().all(|c| c.is_ascii_lowercase()));
    }
}
