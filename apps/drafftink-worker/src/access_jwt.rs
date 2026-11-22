//! The pure part of the Cloudflare Access JWT check (design `docs/design/collab-e2e.md`, section 19.5).
//!
//! The Worker validates the `Cf-Access-Jwt-Assertion` token a second time, so a misconfigured Access application
//! cannot open the instance. The work is split in three so that nothing here needs a clock, a network or an RSA
//! library:
//!
//! 1. [`parse`] splits the token, refuses every algorithm but `RS256` and reads the claims.
//! 2. The TypeScript shell (`src/access.ts`) picks the key by [`Parsed::kid`] and checks the signature with Web Crypto.
//!    An RSA crate in wasm would cost milliseconds, and the Free plan gives a Worker 10 ms of CPU.
//! 3. [`Parsed::check_claims`] compares issuer, audience and times against the clock the host passes in.
//!
//! Every input is hostile. Nothing here panics, and no value that identifies a person is ever printed:
//! [`Identity`] and [`Parsed`] print a placeholder in place of an email or a client id.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

/// The largest token accepted, in bytes. A hostile header cannot make the Worker allocate more than this.
pub const MAX_TOKEN_BYTES: usize = 8 * 1024;

/// The largest accepted base64url header part, in bytes. A real header (`alg`, `kid`, `typ`) is under 200 bytes.
pub const MAX_HEADER_BYTES: usize = 1024;

/// How far into the future an `nbf` claim may lie before the token is refused as "not yet valid": a small allowance
/// for clocks that differ by a few seconds. The `exp` claim gets no allowance.
pub const CLOCK_SKEW_SECS: f64 = 5.0;

/// The only signature algorithm accepted. `none`, `HS256` and every other name are refused, so a token cannot choose
/// the way it is checked (algorithm confusion).
const ALGORITHM: &str = "RS256";

/// The stable word in the JSON body of a refusal. A client acts on this word, never on the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// No usable token was sent at all.
    AccessRequired,
    /// A token was sent and it is no good (bad signature, wrong audience, expired, ...).
    AccessInvalid,
}

impl Code {
    /// The word as it appears in the body (`"access_required"` or `"access_invalid"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AccessRequired => "access_required",
            Self::AccessInvalid => "access_invalid",
        }
    }
}

/// Why a request was refused. A refusal is a value: it has a stable [`Code`] for the client, a separate
/// [`reason`](Self::reason) word for the logs, and a plain-words [`message`](Self::message) for the person.
///
/// Some variants are decided in TypeScript (the signature, the key set, the origin) and are listed here so that the
/// codes and texts have one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("access refused: {}", self.reason())]
pub enum Refusal {
    /// No token in the header or the cookie.
    MissingToken,
    /// No `Cf-Access-Jwt-Assertion` header, and the request comes from a page that is not ours (cookie fallback).
    ForeignOrigin,
    /// The token is longer than [`MAX_TOKEN_BYTES`] (or its header part longer than [`MAX_HEADER_BYTES`]).
    TooLarge,
    /// The token is not three base64url parts holding JSON of the expected shape.
    Malformed,
    /// The header names an algorithm other than `RS256`.
    BadAlg,
    /// The token's `kid` is not in the key set, also not after one refetch.
    UnknownKid,
    /// The key set could not be fetched, so the token cannot be checked. The request fails closed.
    KeysUnavailable,
    /// The signature does not match the signing input.
    BadSignature,
    /// `iss` is not the team's `https://<team>.cloudflareaccess.com`.
    WrongIss,
    /// `aud` does not contain the application's AUD tag.
    WrongAud,
    /// `exp` is in the past, or missing.
    Expired,
    /// `nbf` is in the future (beyond [`CLOCK_SKEW_SECS`]).
    NotYetValid,
    /// Neither `email` (a person) nor `common_name` (a service token) is present.
    NoIdentity,
    /// The server has no team domain or no AUD tag configured. The request fails closed.
    NotConfigured,
}

impl Refusal {
    /// Every variant, for tests and for [`Refusal::from_reason`].
    const ALL: [Self; 14] = [
        Self::MissingToken,
        Self::ForeignOrigin,
        Self::TooLarge,
        Self::Malformed,
        Self::BadAlg,
        Self::UnknownKid,
        Self::KeysUnavailable,
        Self::BadSignature,
        Self::WrongIss,
        Self::WrongAud,
        Self::Expired,
        Self::NotYetValid,
        Self::NoIdentity,
        Self::NotConfigured,
    ];

    /// The stable code of the response body. Only a request without any token is `access_required`.
    #[must_use]
    pub const fn code(self) -> Code {
        match self {
            Self::MissingToken | Self::ForeignOrigin => Code::AccessRequired,
            _ => Code::AccessInvalid,
        }
    }

    /// The word for the log. It names the check that failed and never contains token or identity data.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::MissingToken => "missing_token",
            Self::ForeignOrigin => "foreign_origin",
            Self::TooLarge => "too_large",
            Self::Malformed => "malformed",
            Self::BadAlg => "bad_alg",
            Self::UnknownKid => "unknown_kid",
            Self::KeysUnavailable => "keys_unavailable",
            Self::BadSignature => "bad_signature",
            Self::WrongIss => "wrong_iss",
            Self::WrongAud => "wrong_aud",
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::NoIdentity => "no_identity",
            Self::NotConfigured => "not_configured",
        }
    }

    /// The refusal with this log word, or `None` for a word that is not one of ours.
    #[must_use]
    pub fn from_reason(reason: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|refusal| refusal.reason() == reason)
    }

    /// The plain-words text of the response body (section 19.5). `app_hostname` is the host people open in the
    /// browser, for example `canvas.patwie.com`.
    #[must_use]
    pub fn message(self, app_hostname: &str) -> String {
        match self.code() {
            Code::AccessRequired => format!(
                "This server is only open to signed-in, approved users. Open https://{app_hostname} in your browser \
                 and sign in with Google. In the desktop app, enter the access token for this server in Preferences."
            ),
            Code::AccessInvalid => "Your sign-in is not valid for this server or has expired. Reload the page and \
                                    sign in again."
                .to_owned(),
        }
    }
}

/// A value that identifies a person or a machine. It has no `Display` and prints a placeholder with `Debug`, so a
/// log line or a panic message cannot leak it. [`expose`](Self::expose) is the one way to read it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Private(String);

impl Private {
    /// The value itself. Call this only where the value is needed, never to print it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Private {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<hidden>")
    }
}

/// Who the verified token belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// A person who signed in through the browser: the token has an `email`.
    Person {
        /// The account's email address.
        email: Private,
    },
    /// A service token (desktop, headless, MCP): the token has a `common_name`, the client id, and no `email`.
    Machine {
        /// The service token's client id.
        client_id: Private,
    },
}

impl Identity {
    /// `"person"` or `"machine"`: the only part of the identity that is safe to hand around.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Person { .. } => "person",
            Self::Machine { .. } => "machine",
        }
    }
}

/// What the Worker expects of every token: set from `ACCESS_TEAM_DOMAIN` and `ACCESS_AUD`.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    team_domain: &'a str,
    aud: &'a str,
}

impl<'a> Expected<'a> {
    /// `team_domain` is the host `<team>.cloudflareaccess.com`, without scheme or slash. `aud` is the application's
    /// AUD tag. An empty or malformed value is [`Refusal::NotConfigured`], so a missing setting can never match a
    /// token that has an empty claim.
    ///
    /// # Errors
    /// [`Refusal::NotConfigured`] when either value is empty or the team domain is not a bare host.
    pub fn new(team_domain: &'a str, aud: &'a str) -> Result<Self, Refusal> {
        let bare_host =
            !team_domain.is_empty() && !team_domain.chars().any(|c| c == '/' || c == ':' || c.is_whitespace());
        if bare_host && !aud.is_empty() { Ok(Self { team_domain, aud }) } else { Err(Refusal::NotConfigured) }
    }
}

/// The audience claim: one string, or a list of them.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, aud: &str) -> bool {
        match self {
            Self::One(one) => one == aud,
            Self::Many(many) => many.iter().any(|candidate| candidate == aud),
        }
    }
}

/// The claims that are read. Everything else in the payload is ignored.
#[derive(Debug, Deserialize)]
struct Claims {
    iss: Option<String>,
    aud: Option<Audience>,
    exp: Option<f64>,
    nbf: Option<f64>,
    email: Option<Private>,
    common_name: Option<Private>,
}

/// The JOSE header fields that are read.
#[derive(Debug, Deserialize)]
struct Header {
    alg: Option<String>,
    typ: Option<String>,
    kid: Option<String>,
}

/// A token that has the right shape and uses `RS256`. Its signature is not checked yet, so its claims are not
/// trusted until the host has verified [`signing_input`](Self::signing_input) against
/// [`signature`](Self::signature) with the key named by [`kid`](Self::kid).
#[derive(Debug)]
pub struct Parsed {
    kid: String,
    signing_input: Vec<u8>,
    signature: Vec<u8>,
    claims: Claims,
}

impl Parsed {
    /// The `kid` of the signing key named in the header.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The bytes the signature covers: the header part, a dot and the payload part, as sent.
    #[must_use]
    pub fn signing_input(&self) -> &[u8] {
        &self.signing_input
    }

    /// The raw signature bytes.
    #[must_use]
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    /// Checks the claims against what this server expects, at `now_secs` (seconds since the Unix epoch, passed in so
    /// that nothing here reads a clock). Call it after the signature has been verified.
    ///
    /// The order is issuer, audience, expiry, not-before, identity.
    ///
    /// # Errors
    /// The [`Refusal`] of the first check that fails.
    pub fn check_claims(&self, now_secs: f64, expected: &Expected<'_>) -> Result<Identity, Refusal> {
        if !now_secs.is_finite() {
            return Err(Refusal::Malformed);
        }
        let claims = &self.claims;
        let issuer_is_ours = claims
            .iss
            .as_deref()
            .and_then(|iss| iss.strip_prefix("https://"))
            .is_some_and(|host| host == expected.team_domain);
        if !issuer_is_ours {
            return Err(Refusal::WrongIss);
        }
        if !claims.aud.as_ref().is_some_and(|aud| aud.contains(expected.aud)) {
            return Err(Refusal::WrongAud);
        }
        // A token without `exp` never expires, which Access does not issue: refuse it as expired.
        if !claims.exp.is_some_and(|exp| exp > now_secs) {
            return Err(Refusal::Expired);
        }
        if claims.nbf.is_some_and(|nbf| nbf > now_secs + CLOCK_SKEW_SECS) {
            return Err(Refusal::NotYetValid);
        }
        let non_empty = |value: &Option<Private>| value.as_ref().filter(|v| !v.expose().is_empty()).cloned();
        match (non_empty(&claims.email), non_empty(&claims.common_name)) {
            (Some(email), _) => Ok(Identity::Person { email }),
            (None, Some(client_id)) => Ok(Identity::Machine { client_id }),
            (None, None) => Err(Refusal::NoIdentity),
        }
    }
}

/// Splits and decodes one token and refuses everything that is not an `RS256` token of the expected shape.
///
/// This does not check the signature. See [`Parsed`].
///
/// # Errors
/// [`Refusal::TooLarge`], [`Refusal::Malformed`] or [`Refusal::BadAlg`].
pub fn parse(token: &str) -> Result<Parsed, Refusal> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(Refusal::TooLarge);
    }
    let mut parts = token.split('.');
    let (Some(header_part), Some(payload_part), Some(signature_part), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Refusal::Malformed);
    };
    if header_part.len() > MAX_HEADER_BYTES {
        return Err(Refusal::TooLarge);
    }
    let header: Header = decode_json(header_part)?;
    // The algorithm is checked before anything else of the token is looked at.
    if header.alg.as_deref() != Some(ALGORITHM) {
        return Err(Refusal::BadAlg);
    }
    if header.typ.as_deref().is_some_and(|typ| !typ.eq_ignore_ascii_case("JWT")) {
        return Err(Refusal::Malformed);
    }
    let kid = header.kid.filter(|kid| !kid.is_empty()).ok_or(Refusal::Malformed)?;
    let claims: Claims = decode_json(payload_part)?;
    let signature = URL_SAFE_NO_PAD.decode(signature_part).map_err(|_| Refusal::Malformed)?;
    if signature.is_empty() {
        return Err(Refusal::Malformed);
    }
    let signed_len = header_part.len() + 1 + payload_part.len();
    let signing_input = token.as_bytes().get(..signed_len).ok_or(Refusal::Malformed)?.to_vec();
    Ok(Parsed { kid, signing_input, signature, claims })
}

/// Decodes one base64url part (no padding) and reads it as JSON.
fn decode_json<T: for<'de> Deserialize<'de>>(part: &str) -> Result<T, Refusal> {
    let bytes = URL_SAFE_NO_PAD.decode(part).map_err(|_| Refusal::Malformed)?;
    serde_json::from_slice(&bytes).map_err(|_| Refusal::Malformed)
}

/// The `wasm-bindgen` surface the TypeScript shell calls. Every function returns a plain JS object, and a failure is
/// `{ ok: false, code, reason }`: nothing here throws.
#[cfg(target_arch = "wasm32")]
mod wasm {
    use js_sys::{Object, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};

    use super::{Expected, Refusal, parse};

    fn set(object: &Object, key: &str, value: &JsValue) {
        // `Reflect::set` on a fresh plain object cannot fail; there is nothing to do with an error if it did.
        let _ = Reflect::set(object, &JsValue::from_str(key), value);
    }

    fn failure(refusal: Refusal) -> JsValue {
        let object = Object::new();
        set(&object, "ok", &JsValue::FALSE);
        set(&object, "code", &JsValue::from_str(refusal.code().as_str()));
        set(&object, "reason", &JsValue::from_str(refusal.reason()));
        object.unchecked_into()
    }

    /// `{ ok: true, kid, signingInput: Uint8Array, signature: Uint8Array }` or `{ ok: false, code, reason }`.
    #[wasm_bindgen]
    #[must_use]
    pub fn access_parse(token: &str) -> JsValue {
        match parse(token) {
            Ok(parsed) => {
                let object = Object::new();
                set(&object, "ok", &JsValue::TRUE);
                set(&object, "kid", &JsValue::from_str(parsed.kid()));
                set(&object, "signingInput", &Uint8Array::from(parsed.signing_input()));
                set(&object, "signature", &Uint8Array::from(parsed.signature()));
                object.unchecked_into()
            }
            Err(refusal) => failure(refusal),
        }
    }

    /// Parses `token` again and checks its claims at `now_secs`. `{ ok: true, identity: "person" | "machine" }` or
    /// `{ ok: false, code, reason }`. Call it only after the signature has been verified.
    #[wasm_bindgen]
    #[must_use]
    pub fn access_check_claims(token: &str, now_secs: f64, team_domain: &str, aud: &str) -> JsValue {
        let checked = Expected::new(team_domain, aud)
            .and_then(|expected| parse(token).and_then(|parsed| parsed.check_claims(now_secs, &expected)));
        match checked {
            Ok(identity) => {
                let object = Object::new();
                set(&object, "ok", &JsValue::TRUE);
                set(&object, "identity", &JsValue::from_str(identity.kind()));
                object.unchecked_into()
            }
            Err(refusal) => failure(refusal),
        }
    }

    /// `{ code, reason, message }` for a refusal named by its log word, for the refusals the shell decides itself
    /// (`bad_signature`, `unknown_kid`, `keys_unavailable`, `foreign_origin`, `missing_token`, `not_configured`).
    /// An unknown word gives the `malformed` refusal, so the answer is always a refusal.
    #[wasm_bindgen]
    #[must_use]
    pub fn access_refusal(reason: &str, app_hostname: &str) -> JsValue {
        let refusal = Refusal::from_reason(reason).unwrap_or(Refusal::Malformed);
        let object = Object::new();
        set(&object, "code", &JsValue::from_str(refusal.code().as_str()));
        set(&object, "reason", &JsValue::from_str(refusal.reason()));
        set(&object, "message", &JsValue::from_str(&refusal.message(app_hostname)));
        object.unchecked_into()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::{Value, json};

    use super::*;

    const TEAM: &str = "acme.cloudflareaccess.com";
    const AUD: &str = "aud-tag-1234";
    const NOW: f64 = 1_800_000_000.0;
    const EMAIL: &str = "alice@example.com";
    const CLIENT_ID: &str = "0123456789abcdef.access";

    fn b64(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Builds `header.payload.signature` from JSON values. The signature is a fixed dummy: this module never checks
    /// it (the shell does, and `test/access.test.ts` signs with real keys).
    fn token_of(header: &Value, payload: &Value, signature: &[u8]) -> String {
        format!("{}.{}.{}", b64(header.to_string().as_bytes()), b64(payload.to_string().as_bytes()), b64(signature))
    }

    fn header() -> Value {
        json!({"alg": "RS256", "kid": "key-1", "typ": "JWT"})
    }

    fn person_claims() -> Value {
        json!({
            "iss": format!("https://{TEAM}"),
            "aud": [AUD],
            "exp": NOW + 600.0,
            "iat": NOW - 60.0,
            "nbf": NOW - 60.0,
            "email": EMAIL,
            "type": "app",
        })
    }

    fn machine_claims() -> Value {
        json!({
            "iss": format!("https://{TEAM}"),
            "aud": [AUD],
            "exp": NOW + 600.0,
            "common_name": CLIENT_ID,
            "type": "app",
        })
    }

    fn with(claims: Value, key: &str, value: Value) -> Value {
        let mut claims = claims;
        claims[key] = value;
        claims
    }

    fn without(claims: Value, key: &str) -> Value {
        let mut claims = claims;
        if let Some(object) = claims.as_object_mut() {
            object.remove(key);
        }
        claims
    }

    fn expected() -> Expected<'static> {
        Expected::new(TEAM, AUD).expect("test config is valid")
    }

    fn check(header: &Value, claims: &Value) -> Result<Identity, Refusal> {
        parse(&token_of(header, claims, b"signature-bytes"))?.check_claims(NOW, &expected())
    }

    fn refused_by_parse(token: &str) -> Refusal {
        parse(token).expect_err("the token must be refused")
    }

    #[test]
    fn valid_person_token_passes() {
        let identity = check(&header(), &person_claims()).expect("valid token");
        assert_eq!(identity.kind(), "person");
        assert_eq!(identity, Identity::Person { email: Private(EMAIL.to_owned()) });
    }

    #[test]
    fn service_token_with_common_name_and_no_email_is_a_machine() {
        let identity = check(&header(), &machine_claims()).expect("valid service token");
        assert_eq!(identity.kind(), "machine");
        assert_eq!(identity, Identity::Machine { client_id: Private(CLIENT_ID.to_owned()) });
    }

    #[test]
    fn email_wins_when_both_identity_claims_are_present() {
        let claims = with(person_claims(), "common_name", json!(CLIENT_ID));
        assert_eq!(check(&header(), &claims).expect("valid").kind(), "person");
    }

    #[test]
    fn empty_email_falls_back_to_common_name() {
        let claims = with(machine_claims(), "email", json!(""));
        assert_eq!(check(&header(), &claims).expect("valid").kind(), "machine");
    }

    #[test]
    fn audience_may_be_one_string_or_a_list() {
        let one = with(person_claims(), "aud", json!(AUD));
        assert!(check(&header(), &one).is_ok());
        let many = with(person_claims(), "aud", json!(["other", AUD]));
        assert!(check(&header(), &many).is_ok());
    }

    #[test]
    fn wrong_audience_is_refused() {
        for aud in [json!("other"), json!(["a", "b"]), json!([]), json!(""), json!(null), json!(7)] {
            let claims = with(person_claims(), "aud", aud.clone());
            let refusal = check(&header(), &claims).expect_err("wrong audience");
            // A value of the wrong JSON type cannot be read as an audience at all.
            let wanted = if aud.is_number() { Refusal::Malformed } else { Refusal::WrongAud };
            assert_eq!(refusal, wanted, "aud {aud}");
        }
        let missing = without(person_claims(), "aud");
        assert_eq!(check(&header(), &missing), Err(Refusal::WrongAud));
    }

    #[test]
    fn wrong_issuer_is_refused() {
        for iss in [
            "https://evil.cloudflareaccess.com",
            "http://acme.cloudflareaccess.com",
            "https://acme.cloudflareaccess.com/",
            "https://acme.cloudflareaccess.com.evil.example",
            "acme.cloudflareaccess.com",
            "",
        ] {
            let claims = with(person_claims(), "iss", json!(iss));
            assert_eq!(check(&header(), &claims), Err(Refusal::WrongIss), "iss {iss:?}");
        }
        assert_eq!(check(&header(), &without(person_claims(), "iss")), Err(Refusal::WrongIss));
    }

    #[test]
    fn expired_token_is_refused() {
        for exp in [NOW - 1.0, NOW, 0.0] {
            let claims = with(person_claims(), "exp", json!(exp));
            assert_eq!(check(&header(), &claims), Err(Refusal::Expired), "exp {exp}");
        }
        assert_eq!(check(&header(), &with(person_claims(), "exp", json!(NOW + 1.0))).map(|i| i.kind()), Ok("person"));
    }

    #[test]
    fn token_without_expiry_is_refused() {
        assert_eq!(check(&header(), &without(person_claims(), "exp")), Err(Refusal::Expired));
    }

    #[test]
    fn not_before_in_the_future_is_refused_beyond_the_skew() {
        let far = with(person_claims(), "nbf", json!(NOW + CLOCK_SKEW_SECS + 1.0));
        assert_eq!(check(&header(), &far), Err(Refusal::NotYetValid));
        let near = with(person_claims(), "nbf", json!(NOW + CLOCK_SKEW_SECS));
        assert!(check(&header(), &near).is_ok(), "within the allowed skew");
        let past = with(person_claims(), "nbf", json!(NOW - 1.0));
        assert!(check(&header(), &past).is_ok());
        assert!(check(&header(), &without(person_claims(), "nbf")).is_ok(), "nbf is optional");
    }

    #[test]
    fn token_without_identity_is_refused() {
        let none = without(without(person_claims(), "email"), "common_name");
        assert_eq!(check(&header(), &none), Err(Refusal::NoIdentity));
        let empty = with(with(person_claims(), "email", json!("")), "common_name", json!(""));
        assert_eq!(check(&header(), &empty), Err(Refusal::NoIdentity));
        let null = with(without(person_claims(), "email"), "common_name", json!(null));
        assert_eq!(check(&header(), &null), Err(Refusal::NoIdentity));
    }

    #[test]
    fn claims_are_checked_in_a_fixed_order() {
        let claims = with(
            with(with(without(person_claims(), "email"), "iss", json!("https://x")), "aud", json!("x")),
            "exp",
            json!(1),
        );
        assert_eq!(check(&header(), &claims), Err(Refusal::WrongIss));
        let claims = with(with(without(person_claims(), "email"), "aud", json!("x")), "exp", json!(1));
        assert_eq!(check(&header(), &claims), Err(Refusal::WrongAud));
        let claims = with(without(person_claims(), "email"), "exp", json!(1));
        assert_eq!(check(&header(), &claims), Err(Refusal::Expired));
        let claims = with(without(person_claims(), "email"), "nbf", json!(NOW + 100.0));
        assert_eq!(check(&header(), &claims), Err(Refusal::NotYetValid));
    }

    #[test]
    fn a_clock_that_is_not_a_number_refuses() {
        let parsed = parse(&token_of(&header(), &person_claims(), b"s")).expect("parse");
        for now in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(parsed.check_claims(now, &expected()), Err(Refusal::Malformed));
        }
    }

    #[test]
    fn algorithm_none_is_refused() {
        let token =
            format!("{}.{}.", b64(br#"{"alg":"none","kid":"key-1"}"#), b64(person_claims().to_string().as_bytes()));
        assert_eq!(refused_by_parse(&token), Refusal::BadAlg);
        let with_signature = token_of(&json!({"alg": "none", "kid": "key-1"}), &person_claims(), b"x");
        assert_eq!(refused_by_parse(&with_signature), Refusal::BadAlg);
    }

    #[test]
    fn hs256_signed_with_the_public_key_is_refused() {
        // The classic algorithm confusion: the attacker signs with HMAC, using the public key as the secret. The
        // signature bytes do not matter, because the algorithm is refused before anything else is looked at.
        let token = token_of(&json!({"alg": "HS256", "kid": "key-1", "typ": "JWT"}), &person_claims(), &[7; 32]);
        assert_eq!(refused_by_parse(&token), Refusal::BadAlg);
    }

    #[test]
    fn every_algorithm_but_rs256_is_refused() {
        for alg in [
            "HS256", "HS384", "HS512", "RS384", "RS512", "ES256", "PS256", "EdDSA", "none", "None", "rs256", "RS256 ",
            "",
        ] {
            let header = json!({"alg": alg, "kid": "key-1"});
            assert_eq!(refused_by_parse(&token_of(&header, &person_claims(), b"s")), Refusal::BadAlg, "alg {alg:?}");
        }
        for header in [json!({"kid": "key-1"}), json!({"alg": null, "kid": "key-1"})] {
            assert_eq!(refused_by_parse(&token_of(&header, &person_claims(), b"s")), Refusal::BadAlg);
        }
        assert_eq!(
            refused_by_parse(&token_of(&json!({"alg": 256, "kid": "key-1"}), &person_claims(), b"s")),
            Refusal::Malformed
        );
    }

    #[test]
    fn header_needs_a_kid_and_a_jwt_type() {
        for header in [
            json!({"alg": "RS256"}),
            json!({"alg": "RS256", "kid": ""}),
            json!({"alg": "RS256", "kid": null}),
            json!({"alg": "RS256", "kid": 5}),
            json!({"alg": "RS256", "kid": "k", "typ": "JWE"}),
            json!(["RS256"]),
            json!("RS256"),
        ] {
            assert_eq!(refused_by_parse(&token_of(&header, &person_claims(), b"s")), Refusal::Malformed, "{header}");
        }
        let no_typ = json!({"alg": "RS256", "kid": "k"});
        assert!(parse(&token_of(&no_typ, &person_claims(), b"s")).is_ok(), "typ is optional");
        let lower_typ = json!({"alg": "RS256", "kid": "k", "typ": "jwt"});
        assert!(parse(&token_of(&lower_typ, &person_claims(), b"s")).is_ok());
    }

    #[test]
    fn payload_must_be_a_json_object_of_the_expected_types() {
        for payload in [
            json!([]),
            json!("x"),
            json!(5),
            json!(null),
            json!({"exp": "soon"}),
            json!({"iss": 5}),
            json!({"email": 5}),
        ] {
            assert_eq!(refused_by_parse(&token_of(&header(), &payload, b"s")), Refusal::Malformed, "{payload}");
        }
    }

    #[test]
    fn parse_exposes_what_the_shell_needs_to_verify() {
        let token = token_of(&header(), &person_claims(), &[1, 2, 3, 250, 251]);
        let parsed = parse(&token).expect("parse");
        assert_eq!(parsed.kid(), "key-1");
        assert_eq!(parsed.signature(), [1, 2, 3, 250, 251]);
        let signed = token.rsplit_once('.').expect("two dots").0;
        assert_eq!(parsed.signing_input(), signed.as_bytes());
    }

    #[test]
    fn truncated_and_misshapen_tokens_are_refused() {
        let token = token_of(&header(), &person_claims(), b"signature-bytes");
        let (head, _) = token.split_once('.').expect("dot");
        for broken in [
            String::new(),
            ".".to_owned(),
            "..".to_owned(),
            "a".to_owned(),
            "a.b".to_owned(),
            format!("{token}.extra"),
            format!("{head}."),
            format!("{head}.."),
            token[..10].to_owned(),
            token[..head.len() + 5].to_owned(),
            token.replace('.', "!"),
            format!("{token}="),
            format!(" {token}"),
            format!("{token}\n"),
        ] {
            assert_eq!(refused_by_parse(&broken), Refusal::Malformed, "token {broken:?}");
        }
    }

    #[test]
    fn a_token_cut_inside_the_signature_still_parses_and_is_left_to_the_signature_check() {
        let token = token_of(&header(), &person_claims(), &[9; 256]);
        let cut = &token[..token.len() - 8];
        let parsed = parse(cut).expect("shape is fine");
        assert!(parsed.signature().len() < 256, "the shell refuses it with bad_signature");
    }

    #[test]
    fn oversize_tokens_are_refused_before_decoding() {
        let huge = "a".repeat(MAX_TOKEN_BYTES + 1);
        assert_eq!(refused_by_parse(&huge), Refusal::TooLarge);
        let padded_claims = with(person_claims(), "pad", json!("x".repeat(MAX_TOKEN_BYTES)));
        assert_eq!(refused_by_parse(&token_of(&header(), &padded_claims, b"s")), Refusal::TooLarge);
        let long_header = json!({"alg": "RS256", "kid": "k".repeat(MAX_HEADER_BYTES)});
        assert_eq!(refused_by_parse(&token_of(&long_header, &person_claims(), b"s")), Refusal::TooLarge);
        let edge = "a".repeat(MAX_TOKEN_BYTES);
        assert_eq!(refused_by_parse(&edge), Refusal::Malformed, "exactly the cap is not 'too large'");
    }

    #[test]
    fn config_that_is_empty_or_not_a_host_is_not_configured() {
        for (team, aud) in [
            ("", AUD),
            (TEAM, ""),
            ("https://acme.cloudflareaccess.com", AUD),
            ("acme.cloudflareaccess.com/", AUD),
            ("acme.cloudflareaccess.com:443", AUD),
            (" acme.cloudflareaccess.com", AUD),
        ] {
            assert!(matches!(Expected::new(team, aud), Err(Refusal::NotConfigured)), "{team:?} {aud:?}");
        }
    }

    #[test]
    fn debug_output_never_contains_the_email_or_the_client_id() {
        let person = parse(&token_of(&header(), &person_claims(), b"s")).expect("parse");
        let machine = parse(&token_of(&header(), &machine_claims(), b"s")).expect("parse");
        let identities = [
            person.check_claims(NOW, &expected()).expect("valid"),
            machine.check_claims(NOW, &expected()).expect("valid"),
        ];
        let printed = format!("{person:?} {machine:?} {identities:?} {:#?}", identities[0]);
        assert!(!printed.contains(EMAIL) && !printed.contains("alice"), "{printed}");
        assert!(!printed.contains(CLIENT_ID) && !printed.contains("0123456789"), "{printed}");
        assert!(printed.contains("<hidden>"));
    }

    #[test]
    fn codes_and_reasons_are_stable_words() {
        let table = [
            (Refusal::MissingToken, "access_required", "missing_token"),
            (Refusal::ForeignOrigin, "access_required", "foreign_origin"),
            (Refusal::TooLarge, "access_invalid", "too_large"),
            (Refusal::Malformed, "access_invalid", "malformed"),
            (Refusal::BadAlg, "access_invalid", "bad_alg"),
            (Refusal::UnknownKid, "access_invalid", "unknown_kid"),
            (Refusal::KeysUnavailable, "access_invalid", "keys_unavailable"),
            (Refusal::BadSignature, "access_invalid", "bad_signature"),
            (Refusal::WrongIss, "access_invalid", "wrong_iss"),
            (Refusal::WrongAud, "access_invalid", "wrong_aud"),
            (Refusal::Expired, "access_invalid", "expired"),
            (Refusal::NotYetValid, "access_invalid", "not_yet_valid"),
            (Refusal::NoIdentity, "access_invalid", "no_identity"),
            (Refusal::NotConfigured, "access_invalid", "not_configured"),
        ];
        assert_eq!(table.len(), Refusal::ALL.len(), "every refusal is in the table");
        for (refusal, code, reason) in table {
            assert_eq!(refusal.code().as_str(), code);
            assert_eq!(refusal.reason(), reason);
            assert_eq!(Refusal::from_reason(reason), Some(refusal));
            assert_eq!(refusal.to_string(), format!("access refused: {reason}"));
        }
        assert_eq!(Refusal::from_reason("nonsense"), None);
    }

    #[test]
    fn texts_are_the_plain_words_of_the_design() {
        assert_eq!(
            Refusal::MissingToken.message("canvas.patwie.com"),
            "This server is only open to signed-in, approved users. Open https://canvas.patwie.com in your browser and \
             sign in with Google. In the desktop app, enter the access token for this server in Preferences."
        );
        let invalid = "Your sign-in is not valid for this server or has expired. Reload the page and sign in again.";
        for refusal in Refusal::ALL.into_iter().filter(|r| r.code() == Code::AccessInvalid) {
            assert_eq!(refusal.message("canvas.patwie.com"), invalid, "{refusal}");
        }
        assert!(Refusal::ForeignOrigin.message("other.example").contains("https://other.example"));
    }

    #[test]
    fn mutating_a_valid_token_never_panics() {
        let token = token_of(&header(), &person_claims(), b"signature-bytes");
        for index in 0..token.len() {
            for replacement in ['.', 'A', '_', '-', '=', '\u{e9}', '\0'] {
                let mut mutated = token.clone();
                mutated.replace_range(index..=index, &replacement.to_string());
                if let Ok(parsed) = parse(&mutated) {
                    let _ = parsed.check_claims(NOW, &expected());
                }
            }
        }
    }

    proptest! {
        #[test]
        fn random_strings_never_panic(token in any::<String>()) {
            if let Ok(parsed) = parse(&token) {
                let _ = parsed.check_claims(NOW, &expected());
            }
        }

        #[test]
        fn random_dotted_base64_never_panics(
            a in "[A-Za-z0-9_-]{0,200}",
            b in "[A-Za-z0-9_=-]{0,400}",
            c in "[A-Za-z0-9_+/=-]{0,200}",
        ) {
            if let Ok(parsed) = parse(&format!("{a}.{b}.{c}")) {
                let _ = parsed.check_claims(NOW, &expected());
            }
        }

        #[test]
        fn random_json_parts_never_panic(
            header in any::<Vec<u8>>(),
            payload in any::<Vec<u8>>(),
            signature in any::<Vec<u8>>(),
            now in any::<f64>(),
        ) {
            let token = format!("{}.{}.{}", b64(&header), b64(&payload), b64(&signature));
            if let Ok(parsed) = parse(&token) {
                let _ = parsed.check_claims(now, &expected());
            }
        }

        #[test]
        fn random_claim_values_never_pass_with_the_wrong_audience(exp in any::<f64>(), nbf in any::<f64>(), aud in "[a-z]{0,8}") {
            let claims = json!({"iss": format!("https://{TEAM}"), "aud": aud, "exp": exp, "nbf": nbf, "email": EMAIL});
            if let Ok(parsed) = parse(&token_of(&header(), &claims, b"s")) {
                prop_assert_ne!(parsed.check_claims(NOW, &expected()).map(|_| ()), Ok(()));
            }
        }
    }
}
