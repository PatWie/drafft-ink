#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The room link `#room=<id>,<key>`.

mod common;

use common::*;
use drafftink_seal::{LinkError, RoomKey, RoomLink};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

fn link() -> RoomLink {
    RoomLink::new(vector_key())
}

#[test]
fn a_link_formats_and_parses_back() {
    for seed in 0..50u8 {
        let link = RoomLink::new(RoomKey::from_bytes([seed.wrapping_mul(37); 32]));
        let fragment = link.fragment();
        assert!(fragment.starts_with("#room="));
        // 22 characters for the id, a comma, 43 for the key, no padding.
        assert_eq!(fragment.len(), "#room=".len() + 22 + 1 + 43, "{seed}");
        assert_eq!(fragment.matches('=').count(), 1, "no padding");
        let parsed = RoomLink::parse(&fragment).unwrap();
        assert_eq!(parsed.key(), link.key());
        assert_eq!(parsed.id(), link.id());
    }
}

#[test]
fn a_link_is_read_from_an_address_a_fragment_or_the_bare_text() {
    let link = link();
    let fragment = link.fragment();
    let bare = &fragment[1..];
    let url = link.url("https://drafft.ink/");
    assert_eq!(url.as_str(), format!("https://drafft.ink/{}", fragment.as_str()));
    for text in [
        fragment.as_str().to_owned(),
        bare.to_owned(),
        url.as_str().to_owned(),
        format!("  {}\n", url.as_str()),
        format!("http://localhost:3030/app/#room={}", bare.strip_prefix("room=").unwrap()),
        // The server of a self-hosted room travels next to the room, before or after it.
        format!("{}&server=wss%3A%2F%2Fexample.org%2Fws", fragment.as_str()),
        format!("#server=wss%3A%2F%2Fexample.org&{bare}"),
    ] {
        let parsed = RoomLink::parse(&text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(parsed.key(), link.key());
    }
}

#[test]
fn damaged_links_say_what_to_do() {
    let good = link().fragment();
    let (id, key) = good["#room=".len()..].split_once(',').unwrap();
    let other_id = RoomLink::new(RoomKey::from_bytes([9; 32])).id().to_string();
    let cases: Vec<(String, LinkError)> = vec![
        (String::new(), LinkError::NotARoomLink),
        ("https://drafft.ink/".into(), LinkError::NotARoomLink),
        ("#something=else".into(), LinkError::NotARoomLink),
        // A plain (v1) room has a name and no key.
        ("#room=my-room".into(), LinkError::NoKey),
        ("https://drafft.ink/#room=0123456789abcdef0123456789abcdef".into(), LinkError::NoKey),
        (format!("#room=,{key}"), LinkError::BadId),
        (format!("#room={},{key}", &id[..21]), LinkError::BadId),
        (format!("#room={id}x,{key}"), LinkError::BadId),
        (format!("#room={id}=,{key}"), LinkError::BadId),
        (format!("#room={id},"), LinkError::BadKey),
        (format!("#room={id},{}", &key[..42]), LinkError::BadKey),
        (format!("#room={id},{key}A"), LinkError::BadKey),
        (format!("#room={id},{key}="), LinkError::BadKey),
        (format!("#room={id},{}!", &key[..42]), LinkError::BadKey),
        (format!("#room={id},{key},{key}"), LinkError::BadKey),
        // The last character of a 43-character key has two spare bits, which must be zero.
        (format!("#room={id},{}B", &key[..42]), LinkError::BadKey),
        (format!("#room={other_id},{key}"), LinkError::IdMismatch),
    ];
    for (text, expected) in cases {
        let err = RoomLink::parse(&text).err().unwrap_or_else(|| panic!("accepted {text:?}"));
        assert_eq!(err, expected, "{text:?}");
        let words = err.to_string();
        assert!(words.ends_with('.'), "{words}");
        assert!(!words.contains(key) && !words.contains(id), "an error never repeats the link: {words}");
    }
}

#[test]
fn a_key_that_changed_in_one_character_is_refused() {
    let good = link().fragment();
    let start = "#room=".len() + 22 + 1;
    for position in start..good.len() {
        let mut chars: Vec<char> = good.chars().collect();
        chars[position] = if chars[position] == 'A' { 'B' } else { 'A' };
        let text: String = chars.into_iter().collect();
        assert!(RoomLink::parse(&text).is_err(), "position {position}");
    }
}

#[test]
fn no_text_makes_the_parser_panic() {
    let mut rng = Rng(0xfeed);
    let alphabet = b"#room=,&AaBb09-_/:.%\xc3\xa9 \n";
    for _ in 0..20_000 {
        let len = rng.below(120);
        let bytes: Vec<u8> = (0..len).map(|_| alphabet[rng.below(alphabet.len())]).collect();
        if let Ok(text) = String::from_utf8(bytes) {
            let _ = RoomLink::parse(&text);
        }
    }
    let _ = RoomLink::parse(&"room=,".repeat(100_000));
    let _ = RoomLink::parse(&format!("#room={}", "A".repeat(1 << 20)));
}
