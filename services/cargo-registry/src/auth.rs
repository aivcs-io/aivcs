//! Registry authentication.
//!
//! Three accepted shapes for one token:
//!
//! - `Authorization: Bearer <token>` — cargo
//! - `Authorization: <token>` — raw, as some tooling sends it
//! - `Authorization: Basic base64(login:password)` — **required for Nix**
//!
//! The Basic case is not a convenience. Nix authenticates fixed-output
//! derivation fetches via netrc, and curl turns a netrc entry into HTTP Basic.
//! A bearer-only check 401s every hermetic build even with a valid token — that
//! regression cost the fleet four days of failing builds in August 2026
//! (infra-code#1509/#1510). Removing Basic support re-breaks every `nix build`
//! that depends on an aivcs-registry crate.
//!
//! Basic carries the credential as base64 on every request, which is encoding,
//! not encryption. That is why the service refuses plaintext (see
//! `require_https` in `routes`): Basic is load-bearing on TLS.

use base64::Engine;

/// Does this request carry the registry token, in any accepted scheme?
pub fn authorized(auth_header: Option<&str>, token: &str) -> bool {
    let Some(auth) = auth_header else {
        return false;
    };

    if auth == token {
        return true;
    }
    if let Some(rest) = auth.strip_prefix("Bearer ") {
        if constant_time_eq(rest, token) {
            return true;
        }
    }
    if let Some(rest) = auth.strip_prefix("Basic ") {
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(rest) {
            if let Ok(decoded) = String::from_utf8(decoded) {
                let (login, password) = match decoded.split_once(':') {
                    Some((l, p)) => (l, p),
                    None => (decoded.as_str(), ""),
                };
                // netrc is written `login token password <TOKEN>`, but accept the
                // token in either field so a `login <TOKEN>` entry also works.
                if constant_time_eq(password, token) || constant_time_eq(login, token) {
                    return true;
                }
            }
        }
    }
    false
}

/// Length-independent comparison, so a wrong token cannot be recovered by timing.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "s3cr3t-token";

    fn basic(login: &str, password: &str) -> String {
        let raw = format!("{login}:{password}");
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw)
        )
    }

    #[test]
    fn accepts_bearer() {
        assert!(authorized(Some(&format!("Bearer {TOKEN}")), TOKEN));
    }

    #[test]
    fn accepts_raw_token() {
        assert!(authorized(Some(TOKEN), TOKEN));
    }

    #[test]
    fn accepts_basic_with_token_as_password() {
        // The netrc shape: `machine … login token password <TOKEN>`.
        assert!(authorized(Some(&basic("token", TOKEN)), TOKEN));
    }

    #[test]
    fn accepts_basic_with_token_as_login() {
        assert!(authorized(Some(&basic(TOKEN, "")), TOKEN));
    }

    #[test]
    fn rejects_missing_wrong_and_malformed() {
        assert!(!authorized(None, TOKEN));
        assert!(!authorized(Some("Bearer nope"), TOKEN));
        assert!(!authorized(Some(&basic("token", "nope")), TOKEN));
        assert!(!authorized(Some("Basic !!!not-base64!!!"), TOKEN));
        assert!(!authorized(Some("Basic"), TOKEN));
        assert!(!authorized(Some(""), TOKEN));
    }

    #[test]
    fn rejects_a_prefix_of_the_token() {
        // Guards against a length-truncating comparison.
        assert!(!authorized(Some(&format!("Bearer {}", &TOKEN[..4])), TOKEN));
    }
}
