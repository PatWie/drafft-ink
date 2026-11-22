#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The key, the token and the link never appear in `Debug` output, in an error or in a log line.
//!
//! Every value that holds a secret is printed with `{:?}` and `{:#?}`, and so is every error that a bad link, a bad
//! record or a bad frame can make. The output is searched for the secrets in every form a log line could hold them.

mod common;

use base64ct::{Base64, Base64Unpadded, Base64Url, Base64UrlUnpadded, Encoding};
use common::*;
use drafftink_protocol::ClientId;
use drafftink_protocol::v2::{ClientMessage, JoinMode, Proof, RecordHeader, RecordKind};
use drafftink_seal::{LinkError, Purpose, RoomKey, RoomKeys, RoomLink, SealedRecord};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// A key that is not 0, 1, 2, ... so that its decimal form does not occur by chance.
fn secret_key() -> RoomKey {
    RoomKey::from_bytes(std::array::from_fn(|i| (i as u8).wrapping_mul(53).wrapping_add(171)))
}

/// Every way a program prints bytes: hex, the four base64 alphabets, and the decimal list of a `Debug` array. The
/// whole value and its first half, so that a print that cuts the secret short is found too.
fn spellings(bytes: &[u8]) -> Vec<String> {
    let whole = |b: &[u8]| {
        vec![
            hex(b),
            hex(b).to_uppercase(),
            Base64::encode_string(b),
            Base64Unpadded::encode_string(b),
            Base64Url::encode_string(b),
            Base64UrlUnpadded::encode_string(b),
            format!("{b:?}").trim_matches(['[', ']']).to_owned(),
        ]
    };
    let half = &bytes[..bytes.len() / 2];
    let mut all = whole(bytes);
    // The half of the base64 forms is not a prefix of the whole when the cut is not on a 3-byte boundary, so it is
    // spelled from the bytes. The decimal form of half the bytes is a prefix of the decimal form of all of them.
    all.extend(whole(&half[..half.len() - half.len() % 3]));
    all
}

struct Secrets {
    names: Vec<(&'static str, Vec<String>)>,
}

impl Secrets {
    fn new() -> Self {
        let key = secret_key();
        let link = RoomLink::new(secret_key());
        let mut names = vec![
            ("room key", spellings(key.expose_bytes())),
            ("membership token", spellings(key.membership_token().expose_bytes())),
            ("link", vec![link.fragment().as_str().to_owned(), link.fragment()["#room=".len()..].to_owned()]),
        ];
        for (name, purpose) in [
            ("content key", Purpose::Content),
            ("presence key", Purpose::Presence),
            ("image key", Purpose::Image),
            ("image-name key", Purpose::ImageName),
        ] {
            names.push((name, spellings(key.subkey(purpose).expose_bytes())));
        }
        Secrets { names }
    }

    /// The secret and the form of it that `output` shows, if it shows one.
    fn find_leak(&self, output: &str) -> Option<String> {
        self.names.iter().find_map(|(name, forms)| {
            let form = forms.iter().find(|form| output.contains(form.as_str()))?;
            Some(format!("the {name} as {form:?}"))
        })
    }

    #[track_caller]
    fn assert_none_in(&self, what: &str, output: &str) {
        if let Some(leak) = self.find_leak(output) {
            panic!("{what} shows {leak}: {output}");
        }
    }
}

fn shown(value: &dyn std::fmt::Debug) -> String {
    format!("{value:?}\n{value:#?}")
}

#[test]
fn the_search_finds_a_secret_that_is_printed() {
    // A search that finds nothing would pass every test below. This shows that it finds a leak.
    let secrets = Secrets::new();
    let key = secret_key();
    let leaks = [
        format!("{:?}", key.expose_bytes()),
        hex(key.expose_bytes()),
        Base64UrlUnpadded::encode_string(key.membership_token().expose_bytes()),
        format!("key {}", RoomLink::new(secret_key()).fragment().as_str()),
        format!("{:?}", &key.expose_bytes()[..16]),
    ];
    for leak in leaks {
        assert!(secrets.find_leak(&leak).is_some(), "{leak} was not found");
    }
    assert!(secrets.find_leak("RoomKey(..) RoomLink { id: 4BnGQbM5wKmM6EC15PDNvg, .. }").is_none());
}

#[test]
fn no_type_that_holds_a_secret_prints_it() {
    let secrets = Secrets::new();
    let key = secret_key();
    let keys = RoomKeys::new(&key);
    let token = key.membership_token();
    let link = RoomLink::new(secret_key());
    let join = ClientMessage::Join {
        min_version: 2,
        max_version: 2,
        room: key.room_id(),
        client_id: client(1),
        mode: JoinMode::Create,
        proof: Proof::token(&token),
        last_seq: 0,
        captcha: None,
    };
    let sealed = keys.seal(RecordHeader::new(RecordKind::Update, client(1), 1, 0).unwrap(), b"payload").unwrap();

    secrets.assert_none_in("RoomKey", &shown(&key));
    secrets.assert_none_in("Subkey", &shown(&key.subkey(Purpose::Content)));
    secrets.assert_none_in("RoomKeys", &shown(&keys));
    secrets.assert_none_in("MembershipToken", &shown(&token));
    secrets.assert_none_in("Proof", &shown(&Proof::token(&token)));
    secrets.assert_none_in("RoomLink", &shown(&link));
    secrets.assert_none_in("Join", &shown(&join));
    secrets.assert_none_in("Join clone", &shown(&join.clone()));
    secrets.assert_none_in("Option<RoomKey>", &shown(&Some(secret_key())));
    secrets.assert_none_in("Vec<RoomKey>", &shown(&vec![secret_key(), secret_key()]));
    secrets.assert_none_in("a parsed link", &shown(&RoomLink::parse(link.fragment().as_str())));
    secrets.assert_none_in("SealedRecord", &shown(&sealed));
    secrets.assert_none_in("the room id", &format!("{} {:?}", key.room_id(), key.room_id()));
}

#[test]
fn no_error_repeats_a_secret() {
    let secrets = Secrets::new();
    let link = RoomLink::new(secret_key()).fragment();
    let key = secret_key();
    let keys = RoomKeys::new(&key);

    // Every cut and every one-character change of the link makes an error or a link. An error shows no input.
    let chars: Vec<char> = link.chars().collect();
    let mut errors: Vec<LinkError> = Vec::new();
    for cut in 0..chars.len() {
        errors.extend(RoomLink::parse(&chars[..cut].iter().collect::<String>()).err());
        let mut changed = chars.clone();
        changed[cut] = if changed[cut] == '~' { '!' } else { '~' };
        errors.extend(RoomLink::parse(&changed.into_iter().collect::<String>()).err());
    }
    assert!(errors.len() > chars.len(), "the loop made errors");
    for err in errors {
        secrets.assert_none_in("a link error", &format!("{err} {err:?}"));
    }

    // Records that fail to open.
    let sealed = keys.seal(RecordHeader::new(RecordKind::Update, client(1), 1, 0).unwrap(), b"payload").unwrap();
    let other = self::keys(1);
    secrets.assert_none_in("OpenError", &format!("{0} {0:?}", other.open(RecordKind::Update, &sealed).unwrap_err()));
    secrets.assert_none_in("OpenError", &format!("{0:?}", keys.open(RecordKind::Snapshot, &sealed).unwrap_err()));
    let short = SealedRecord::from_bytes(sealed.as_bytes()[..10].to_vec()).unwrap_err();
    secrets.assert_none_in("RecordError", &format!("{short} {short:?}"));
    let too_large =
        keys.seal(RecordHeader::new(RecordKind::Presence, client(1), 1, 0).unwrap(), &[0u8; 1 << 17]).unwrap_err();
    secrets.assert_none_in("SealError", &format!("{too_large} {too_large:?}"));

    // A hostile or damaged Join frame: no decoding error repeats the token that is in it.
    let join = ClientMessage::Join {
        min_version: 2,
        max_version: 2,
        room: key.room_id(),
        client_id: ClientId::from_bytes([1; 16]),
        mode: JoinMode::Open,
        proof: Proof::token(&key.membership_token()),
        last_seq: 7,
        captcha: Some("captcha".into()),
    };
    let frame = join.encode().unwrap();
    for cut in 0..frame.len() {
        if let Err(e) = ClientMessage::decode(&frame[..cut]) {
            secrets.assert_none_in("a truncated Join", &format!("{e} {e:?}"));
        }
    }
    for index in 0..frame.len() {
        let mut bytes = frame.clone();
        bytes[index] ^= 0xff;
        if let Err(e) = ClientMessage::decode(&bytes) {
            secrets.assert_none_in("a damaged Join", &format!("{e} {e:?}"));
        }
    }
}
