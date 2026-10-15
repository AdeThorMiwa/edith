//! Where a provider's key comes from.

use crate::LlmError;
use std::sync::Arc;
use zeroize::Zeroizing;

/// Where the key comes from, asked once per request, and how to drop any copy kept of it. The
/// client never stores the key itself; a source may keep one in wipe-on-drop memory so a
/// conversation costs one keychain prompt instead of one per request.
#[derive(Clone)]
pub struct KeySource {
    pub(crate) get: Arc<dyn Fn() -> Result<Zeroizing<String>, LlmError> + Send + Sync>,
    pub(crate) forget: Arc<dyn Fn() + Send + Sync>,
}

impl KeySource {
    pub fn new(get: impl Fn() -> Result<Zeroizing<String>, LlmError> + Send + Sync + 'static) -> Self {
        Self { get: Arc::new(get), forget: Arc::new(|| {}) }
    }

    /// Called when the provider rejects the key, so a replaced key is read afresh next time.
    pub fn with_forget(mut self, forget: impl Fn() + Send + Sync + 'static) -> Self {
        self.forget = Arc::new(forget);
        self
    }
}
