//! Join authentication: a list of accepted bearer tokens, compared in constant time.

use subtle::ConstantTimeEq;

/// Auth is on when at least one token is configured.
#[derive(Clone, Default)]
pub struct Auth {
    tokens: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Auth({} token(s))", self.tokens.len())
    }
}

impl Auth {
    /// Accepts any of `tokens`. An empty list turns auth off.
    pub fn new(tokens: &[String]) -> Self {
        Auth { tokens: tokens.iter().map(|t| t.as_bytes().to_vec()).collect() }
    }

    /// Whether clients must present a token.
    pub fn enabled(&self) -> bool {
        !self.tokens.is_empty()
    }

    /// Whether `presented` is acceptable. With auth off every value (even none) is. With auth
    /// on, every token is compared, so the time does not show which one matched.
    pub fn check(&self, presented: Option<&str>) -> bool {
        if !self.enabled() {
            return true;
        }
        let Some(presented) = presented else { return false };
        let mut ok = subtle::Choice::from(0);
        for token in &self.tokens {
            ok |= token.as_slice().ct_eq(presented.as_bytes());
        }
        ok.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_accepts_everything() {
        let a = Auth::new(&[]);
        assert!(!a.enabled());
        assert!(a.check(None) && a.check(Some("anything")));
    }

    #[test]
    fn on_accepts_only_listed_tokens() {
        let a = Auth::new(&["alpha".into(), "beta".into()]);
        assert!(a.enabled());
        assert!(a.check(Some("alpha")) && a.check(Some("beta")));
        assert!(!a.check(None));
        assert!(!a.check(Some("")));
        assert!(!a.check(Some("alph")));
        assert!(!a.check(Some("alphaa")));
        assert!(!a.check(Some("ALPHA")));
    }
}
