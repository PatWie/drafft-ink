//! What the Worker may decide before it calls a Durable Object: is this a room id, a picture name and a membership
//! token that belongs to the room. It hashes 32 bytes and nothing else; the Worker never reads a payload.

use std::str::FromStr;

use base64ct::{Base64UrlUnpadded, Encoding};
use drafftink_protocol::v2::{ImageName, MEMBERSHIP_TOKEN_BYTES, MembershipToken, Proof, RoomId};

/// The scheme word of the `Authorization` header: `Drafft-Member <base64url token>`.
pub const AUTH_SCHEME: &str = "Drafft-Member";

/// Why a request does not get to a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EdgeError {
    /// The room id in the path is not 16 bytes of unpadded base64url.
    #[error("the room id in the address is not valid")]
    BadRoom,
    /// The picture name in the path is not 16 bytes of unpadded base64url.
    #[error("the picture name in the address is not valid")]
    BadName,
    /// The `Authorization` header is missing or is not `Drafft-Member <token>`.
    #[error("the request carries no membership token")]
    NoToken,
    /// The token does not belong to this room.
    #[error("the membership token does not belong to this room")]
    WrongRoom,
}

/// The room id of a path segment.
pub fn parse_room(text: &str) -> Result<RoomId, EdgeError> {
    RoomId::from_str(text).map_err(|_| EdgeError::BadRoom)
}

/// The picture name of a path segment.
pub fn parse_name(text: &str) -> Result<ImageName, EdgeError> {
    ImageName::from_str(text).map_err(|_| EdgeError::BadName)
}

/// The membership proof in an `Authorization` header value.
pub fn parse_proof(authorization: Option<&str>) -> Result<Proof, EdgeError> {
    let text = authorization.and_then(|v| v.strip_prefix(AUTH_SCHEME)).ok_or(EdgeError::NoToken)?;
    decode_token(text.trim()).ok_or(EdgeError::NoToken)
}

/// The proof of a bare base64url token (the core takes it from the shell in this form).
pub fn decode_token(text: &str) -> Option<Proof> {
    let mut bytes = [0u8; MEMBERSHIP_TOKEN_BYTES];
    let decoded = Base64UrlUnpadded::decode(text, &mut bytes).ok()?;
    (decoded.len() == MEMBERSHIP_TOKEN_BYTES).then(|| Proof::token(&MembershipToken::from_bytes(bytes)))
}

/// Checks that `authorization` carries a token of the room named by the path segment `room`.
pub fn check_member(room: &str, authorization: Option<&str>) -> Result<RoomId, EdgeError> {
    let room = parse_room(room)?;
    let proof = parse_proof(authorization)?;
    if room.accepts(&proof) { Ok(room) } else { Err(EdgeError::WrongRoom) }
}

/// The id of the room that a bare base64url token belongs to, as it appears in paths.
pub fn room_id_of_token(token: &str) -> Option<String> {
    let mut bytes = [0u8; MEMBERSHIP_TOKEN_BYTES];
    let decoded = Base64UrlUnpadded::decode(token, &mut bytes).ok()?;
    (decoded.len() == MEMBERSHIP_TOKEN_BYTES)
        .then(|| RoomId::from_token(&MembershipToken::from_bytes(bytes)).to_string())
}

/// The R2 key prefix of a room: `r/<room id as hex>/`. Everything the room keeps in R2 lives under it, so a deletion
/// lists this prefix and nothing else.
pub fn r2_prefix(room: &str) -> Result<String, EdgeError> {
    Ok(format!("r/{}/", hex(parse_room(room)?.as_bytes())))
}

/// The R2 key of one picture: `r/<room id hex>/<name hex>`.
pub fn r2_key(room: &str, name: &str) -> Result<String, EdgeError> {
    Ok(format!("{}{}", r2_prefix(room)?, hex(parse_name(name)?.as_bytes())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_text() -> String {
        Base64UrlUnpadded::encode_string(&[9u8; 32])
    }

    fn room_text() -> String {
        RoomId::from_token(&MembershipToken::from_bytes([9; 32])).to_string()
    }

    #[test]
    fn a_member_passes_and_a_stranger_does_not() {
        let header = format!("{AUTH_SCHEME} {}", token_text());
        assert!(check_member(&room_text(), Some(&header)).is_ok());
        let other = format!("{AUTH_SCHEME} {}", Base64UrlUnpadded::encode_string(&[8u8; 32]));
        assert_eq!(check_member(&room_text(), Some(&other)), Err(EdgeError::WrongRoom));
    }

    #[test]
    fn a_missing_or_malformed_header_is_no_token() {
        for header in [
            None,
            Some(""),
            Some("Bearer abc"),
            Some("Drafft-Member"),
            Some("Drafft-Member !!"),
            Some("Drafft-Member AAAA"),
        ] {
            assert_eq!(check_member(&room_text(), header), Err(EdgeError::NoToken), "{header:?}");
        }
        // 33 bytes: too long.
        let long = format!("{AUTH_SCHEME} {}", Base64UrlUnpadded::encode_string(&[9u8; 33]));
        assert_eq!(check_member(&room_text(), Some(&long)), Err(EdgeError::NoToken));
    }

    #[test]
    fn a_bad_room_id_is_refused_before_the_token_is_read() {
        let header = format!("{AUTH_SCHEME} {}", token_text());
        for room in ["", "abc", "zzzzzzzzzzzzzzzzzzzzzz==", "../../etc/passwd", &"a".repeat(23)] {
            assert_eq!(check_member(room, Some(&header)), Err(EdgeError::BadRoom), "{room}");
        }
    }

    #[test]
    fn r2_keys_are_the_hex_of_the_room_and_the_name() {
        let room = RoomId::from_bytes([0xAB; 16]).to_string();
        let name = ImageName::from_bytes([0x01; 16]).to_string();
        assert_eq!(r2_prefix(&room).unwrap(), format!("r/{}/", "ab".repeat(16)));
        assert_eq!(r2_key(&room, &name).unwrap(), format!("r/{}/{}", "ab".repeat(16), "01".repeat(16)));
        assert_eq!(r2_key("nope", &name), Err(EdgeError::BadRoom));
        assert_eq!(r2_key(&room, "nope"), Err(EdgeError::BadName));
        assert!(r2_key(&room, &name).unwrap().starts_with(&r2_prefix(&room).unwrap()));
    }

    #[test]
    fn names_are_checked() {
        assert!(parse_name("AAAAAAAAAAAAAAAAAAAAAA").is_ok());
        assert_eq!(parse_name("AAAA"), Err(EdgeError::BadName));
        assert_eq!(parse_name("../x"), Err(EdgeError::BadName));
    }

    #[test]
    fn the_room_id_of_a_token_matches_the_protocol() {
        assert_eq!(room_id_of_token(&token_text()), Some(room_text()));
        assert_eq!(room_id_of_token("short"), None);
    }
}
