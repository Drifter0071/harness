//! Auth gate for HCI connections.
//!
//! Two tokens are in play:
//!
//! - `boot_token`: written into `harness.control.json`, consumed on the
//!   first successful auth. After consumption, the server rotates
//!   the file with a new boot token and invalidates any existing
//!   session tokens.
//! - `session_token`: issued by the server on first auth. The client
//!   presents it on every subsequent request. Session tokens are
//!   invalidated on rotation.
//!
//! The `AuthState` is shared between the listener (which consumes the
//! boot token) and per-connection tasks (which check session tokens).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use base64::Engine;
use rand::RngCore;

/// Random, opaque session-token identifier. Same shape as `boot_token`
/// (256 bits, base64-URL-safe-no-pad) so the on-the-wire treatment is
/// identical.
pub fn fresh_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Debug)]
pub struct AuthState {
    inner: Mutex<AuthInner>,
}

#[derive(Debug)]
struct AuthInner {
    /// The current boot token. `None` once it's been consumed.
    boot_token: Option<String>,
    /// Live session tokens. Cleared on rotation.
    sessions: HashSet<String>,
}

impl AuthState {
    pub fn new(boot_token: String) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(AuthInner {
                boot_token: Some(boot_token),
                sessions: HashSet::new(),
            }),
        })
    }

    /// Construct an `AuthState` with a fresh, cryptographically random
    /// boot token. Used by `integration::start` because the server
    /// will rotate the token into the connection-info file anyway.
    pub fn new_with_random() -> Arc<Self> {
        Self::new(fresh_token())
    }

    /// Try to consume the boot token. On success returns a fresh session
    /// token; on failure returns `None` (token mismatch OR already used).
    pub fn consume_boot_token(&self, presented: &str) -> Option<String> {
        let mut g = self.inner.lock().ok()?;
        let stored = g.boot_token.as_ref()?;
        if !constant_time_eq(stored.as_bytes(), presented.as_bytes()) {
            return None;
        }
        // Consume the boot token so a second client can't race.
        g.boot_token = None;
        // Issue a new session token.
        let session = fresh_token();
        g.sessions.insert(session.clone());
        Some(session)
    }

    /// Returns true if the presented session token is currently live.
    pub fn is_valid_session(&self, presented: &str) -> bool {
        let Ok(g) = self.inner.lock() else {
            return false;
        };
        g.sessions.contains(presented)
    }

    /// Rotate: discard all session tokens, generate a new boot token,
    /// return it. Callers should re-write the connection-info file.
    pub fn rotate(&self) -> String {
        let mut g = self.inner.lock().expect("auth mutex poisoned");
        g.sessions.clear();
        let new_boot = fresh_token();
        g.boot_token = Some(new_boot.clone());
        new_boot
    }
}

/// Constant-time byte comparison. Avoids a timing oracle on the
/// boot-token check. Length difference returns false without a
/// per-byte loop to keep the call cheap.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_token_consumes_and_issues_session() {
        let boot = fresh_token();
        let state = AuthState::new(boot.clone());
        let session = state
            .consume_boot_token(&boot)
            .expect("first auth succeeds");
        assert!(state.is_valid_session(&session));
        // Boot token is now consumed.
        assert!(state.consume_boot_token(&boot).is_none());
    }

    #[test]
    fn wrong_boot_token_rejected() {
        let state = AuthState::new(fresh_token());
        assert!(state.consume_boot_token("not-the-token").is_none());
    }

    #[test]
    fn rotate_invalidates_sessions_and_emits_new_boot() {
        let boot = fresh_token();
        let state = AuthState::new(boot.clone());
        let session = state.consume_boot_token(&boot).unwrap();
        assert!(state.is_valid_session(&session));
        let new_boot = state.rotate();
        assert_ne!(new_boot, boot);
        assert!(!state.is_valid_session(&session));
        // The new boot token is consumable.
        let new_session = state.consume_boot_token(&new_boot).unwrap();
        assert!(state.is_valid_session(&new_session));
    }

    #[test]
    fn fresh_tokens_are_distinct() {
        let a = fresh_token();
        let b = fresh_token();
        assert_ne!(a, b);
        // 32 bytes → 43 base64-URL-safe-no-pad chars
        assert_eq!(a.len(), 43);
    }
}
