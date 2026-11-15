//! A flag the interface sets to stop a running batch.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// True for clones of the same token.
    pub fn same_as(&self, other: &CancelToken) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_flag_and_new_tokens_do_not() {
        let token = CancelToken::new();
        let clone = token.clone();
        let other = CancelToken::new();
        clone.cancel();
        assert!(token.is_cancelled());
        assert!(!other.is_cancelled());
        assert!(token.same_as(&clone));
        assert!(!token.same_as(&other));
    }
}
