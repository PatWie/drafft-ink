//! The plain-words form of a relay's `Error` (protocol v2).

use drafftink_protocol::v2::ErrorCode;

use crate::status::{Problem, ProblemAction, StopReason};

/// What a person reads and can do. `joining` is true before the first catch-up has finished.
pub(super) fn explain(code: ErrorCode, joining: bool) -> Problem {
    let (text, action, fatal) = match code {
        ErrorCode::Malformed => (
            "The relay could not understand a message from the app. This is a bug. Trying again.",
            ProblemAction::Wait,
            false,
        ),
        ErrorCode::UnsupportedVersion => (
            "This app and the server cannot talk to each other: the server does not host encrypted rooms of this \
             version. Update the app, or ask whoever runs the server to update it.",
            ProblemAction::UpdateApp,
            true,
        ),
        ErrorCode::Unauthorized => (
            "The key in this room link does not open this room, or the link is damaged. Ask for the whole link again.",
            ProblemAction::NewLink,
            true,
        ),
        ErrorCode::Forbidden => {
            ("The server does not allow this. Ask whoever runs the server.", ProblemAction::AskAdmin, true)
        }
        ErrorCode::RoomFull if joining => ("This room is full. Trying again in a moment.", ProblemAction::Wait, false),
        ErrorCode::RoomFull => (
            "This room holds as much as a room may, so the latest change is not shared. It is kept on this device. \
             Save a copy and start a new room from it.",
            ProblemAction::ReduceSize,
            false,
        ),
        ErrorCode::TooLarge => (
            "A change is larger than the server accepts, so it is not shared. Your changes are kept on this device.",
            ProblemAction::ReduceSize,
            false,
        ),
        ErrorCode::RateLimited => ("Sending too fast for the server. Slowing down.", ProblemAction::Wait, false),
        ErrorCode::Replaced => (
            "This room was opened again with the same identity, in another tab or window, so this one stopped \
             sharing. Reload to join again.",
            ProblemAction::Reload,
            true,
        ),
        ErrorCode::NotJoined | ErrorCode::AlreadyJoined => {
            ("The app and the server lost track of their conversation. Reconnecting.", ProblemAction::Wait, false)
        }
        ErrorCode::Unavailable => (
            "The server cannot take requests right now. It may be restarting. Trying again soon.",
            ProblemAction::Wait,
            false,
        ),
        ErrorCode::Internal => ("The server hit an error. Trying again.", ProblemAction::Wait, false),
        ErrorCode::RoomMissing => (
            "This room is gone. Rooms are deleted a while after the last visit, or the link is incomplete. Start an \
             empty room with the same link?",
            ProblemAction::StartNewRoom,
            true,
        ),
        ErrorCode::WrongRoomKind => (
            "This address is a room of another kind than the link says. Use the link as it was shared.",
            ProblemAction::NewLink,
            true,
        ),
        ErrorCode::Conflict => ("Another change got there first. Catching up.", ProblemAction::Wait, false),
        ErrorCode::DailyLimit => (
            "The server has reached its daily limit and starts again later. Your drawing is saved on this device. \
             You can keep working alone, run your own server, or try again later.",
            ProblemAction::Wait,
            false,
        ),
    };
    Problem { code: None, text: text.to_owned(), action, fatal }
}

/// The reason a fatal error stops the client for good.
pub(super) fn stop_reason(code: ErrorCode) -> Option<StopReason> {
    match code {
        ErrorCode::UnsupportedVersion => Some(StopReason::UpdateApp),
        ErrorCode::Unauthorized | ErrorCode::WrongRoomKind => Some(StopReason::Unauthorized),
        ErrorCode::Forbidden => Some(StopReason::Forbidden),
        ErrorCode::Replaced => Some(StopReason::Replaced),
        ErrorCode::RoomMissing => Some(StopReason::RoomMissing),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_has_plain_words_and_the_fatal_ones_stop_the_client() {
        for byte in 0..=255u8 {
            let Some(code) = ErrorCode::from_u8(byte) else { continue };
            for joining in [false, true] {
                let p = explain(code, joining);
                assert!(p.text.ends_with('.') || p.text.ends_with('?'), "{code:?}: {}", p.text);
                assert!(
                    p.text.len() > 20 && !p.text.contains("code") && !p.text.contains("Error"),
                    "{code:?}: {}",
                    p.text
                );
                assert_eq!(p.fatal, stop_reason(code).is_some(), "{code:?}");
                assert!(!(code.retryable() && p.fatal), "{code:?}");
            }
        }
    }
}
