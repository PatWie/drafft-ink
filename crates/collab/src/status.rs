//! What the UI shows: the connection status and the plain-words explanation of every failure.
//!
//! Server error codes and local failures become a [`Problem`]: a sentence that says what happened and what to do, an
//! [`ProblemAction`] the UI can turn into a button, and whether the client has stopped for good.

use drafftink_protocol::ErrorCode;

use crate::access::AccessDenial;

/// Where the connection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnState {
    /// Never started: this document is not shared.
    Idle,
    /// Opening the connection or waiting for the server to accept the join.
    Connecting,
    /// Joined; catching up with the room.
    Syncing,
    /// Joined and in sync; edits flow both ways.
    Online,
    /// The connection dropped; waiting to try again. Edits are kept and will sync.
    Reconnecting,
    /// Stopped. See [`StopReason`].
    Stopped,
}

/// Why the client stopped and will not reconnect by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopReason {
    /// The user left the room.
    Left,
    /// The server needs a token and the given one was missing or wrong.
    Unauthorized,
    /// The server does not allow this client.
    Forbidden,
    /// Client and server share no protocol version.
    UpdateApp,
    /// Another connection with the same id took over.
    Replaced,
    /// The document was written by a newer version of the app.
    DocumentTooNew,
    /// The collaboration data hit an internal error; the document must be reloaded.
    NeedsReload,
    /// The document is larger than the server accepts.
    TooLarge,
    /// The user chose to keep their copy instead of replacing it with the server's.
    Declined,
    /// An end-to-end encrypted room that the server does not have: it was deleted after a period without visitors,
    /// or the link is incomplete.
    RoomMissing,
    /// The server is private and turned the connection away: no access token, or one it did not accept
    /// ([`AccessDenial`]). Retrying with the same token would be refused again.
    AccessDenied,
    /// The room was deleted on request (`collab.delete_room`). The relay closed the connection and holds nothing of it.
    RoomDeleted,
}

/// What the UI can offer next to a [`Problem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProblemAction {
    /// Nothing to do; the client retries by itself.
    Wait,
    /// Ask for a new access token and join again.
    NewToken,
    /// Update the app (or the server).
    UpdateApp,
    /// Reload the document or the page, then join again.
    Reload,
    /// Make the document smaller (remove large images) or raise the server limit.
    ReduceSize,
    /// Save a copy of the file: some changes could not be shared.
    SaveCopy,
    /// Ask whoever runs the server.
    AskAdmin,
    /// The user must choose (see `CollabEvent::NeedsDecision`).
    Decide,
    /// Ask whoever shared the room for the whole link again (it is damaged or belongs to another room).
    NewLink,
    /// Start an empty room under the same link (the room was deleted).
    StartNewRoom,
}

/// A failure in plain words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The server's code, when the problem came from the server.
    pub code: Option<ErrorCode>,
    /// What happened and what to do. One or two sentences.
    pub text: String,
    /// What the UI can offer.
    pub action: ProblemAction,
    /// True when the client stopped and will not retry by itself.
    pub fatal: bool,
}

impl Problem {
    pub(crate) fn local(text: impl Into<String>, action: ProblemAction, fatal: bool) -> Self {
        Problem { code: None, text: text.into(), action, fatal }
    }
}

/// The plain-words form of a server `Error`. `joining` is true before the first sync has finished: a `TooLarge`
/// then means the room itself is too big to open, which no retry fixes.
pub fn explain(code: ErrorCode, joining: bool) -> Problem {
    let (text, action, fatal) = match code {
        ErrorCode::Malformed => (
            "The server could not understand a message from the app. This is a bug. Trying again.",
            ProblemAction::Wait,
            false,
        ),
        ErrorCode::UnsupportedVersion => (
            "This app and the server cannot talk to each other because they use different versions of the sharing \
             protocol. Update the app, or ask whoever runs the server to update it.",
            ProblemAction::UpdateApp,
            true,
        ),
        ErrorCode::Unauthorized => (
            "The server needs an access token and the one given is missing or wrong. Check the token and join again.",
            ProblemAction::NewToken,
            true,
        ),
        ErrorCode::Forbidden => (
            "The server does not allow this app to join. Ask whoever runs the server to allow it.",
            ProblemAction::AskAdmin,
            true,
        ),
        ErrorCode::RoomFull => ("This room is full. Trying again in a moment.", ProblemAction::Wait, false),
        ErrorCode::TooLarge if joining => (
            "This document is larger than the server accepts. Remove large images or ask whoever runs the server to \
             raise the limit.",
            ProblemAction::ReduceSize,
            true,
        ),
        ErrorCode::TooLarge => (
            "A change is larger than the server accepts, so it is not shared. Remove large images or ask whoever runs \
             the server to raise the limit. Your changes are kept on this device.",
            ProblemAction::ReduceSize,
            false,
        ),
        ErrorCode::RateLimited => ("Sending too fast for the server. Slowing down.", ProblemAction::Wait, false),
        ErrorCode::ResyncRequired => ("Fell behind the room. Catching up.", ProblemAction::Wait, false),
        ErrorCode::Replaced => (
            "This room was opened again with the same identity, in another tab or window, so this one stopped \
             sharing. Reload to join again.",
            ProblemAction::Reload,
            true,
        ),
        ErrorCode::NotJoined | ErrorCode::AlreadyJoined => {
            ("The app and the server lost track of their conversation. Reconnecting.", ProblemAction::Wait, false)
        }
        ErrorCode::InvalidUpdate => (
            "The server could not read a change made here, so it is not shared. It is kept on this device. Save a copy \
             of the file to keep it safe.",
            ProblemAction::SaveCopy,
            false,
        ),
        ErrorCode::Unavailable => (
            "The server cannot take requests right now. It may be restarting. Trying again soon.",
            ProblemAction::Wait,
            false,
        ),
        ErrorCode::Internal => ("The server hit an error. Trying again.", ProblemAction::Wait, false),
    };
    Problem { code: Some(code), text: text.to_owned(), action, fatal }
}

/// The plain-words form of a server's gate ([`AccessDenial`]): what happened and what to do. `host` is the server's
/// name as the person knows it. `fatal` is true when the client stops because of it (the connection); a picture request
/// that meets the gate leaves the rest of the room running.
pub fn explain_access(denial: AccessDenial, host: &str, fatal: bool) -> Problem {
    let (text, action) = match denial {
        AccessDenial::Required => (
            format!(
                "This server is private. Open Preferences, Collaboration, and enter the access token for {host}. The \
                 server's owner can create one."
            ),
            ProblemAction::AskAdmin,
        ),
        AccessDenial::Rejected => (
            "The server did not accept the access token. It may have expired or been revoked. Ask the server's owner \
             for a new one."
                .to_owned(),
            ProblemAction::AskAdmin,
        ),
        AccessDenial::SignInExpired => (
            "Your sign-in has expired. Reload the page to sign in again. Your drawing is saved on this device."
                .to_owned(),
            ProblemAction::Reload,
        ),
    };
    Problem { code: None, text, action, fatal }
}

/// The stop reason a fatal server error maps to.
pub(crate) fn stop_reason(code: ErrorCode) -> Option<StopReason> {
    match code {
        ErrorCode::UnsupportedVersion => Some(StopReason::UpdateApp),
        ErrorCode::Unauthorized => Some(StopReason::Unauthorized),
        ErrorCode::Forbidden => Some(StopReason::Forbidden),
        ErrorCode::Replaced => Some(StopReason::Replaced),
        _ => None,
    }
}

/// The connection status for a status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Where the connection is.
    pub state: ConnState,
    /// Why it stopped, when `state` is [`ConnState::Stopped`].
    pub stopped: Option<StopReason>,
    /// One sentence for the user.
    pub text: String,
    /// True while local changes are not yet confirmed by the server (or cannot be sent: offline).
    pub unsynced: bool,
    /// When the next connection attempt starts, while reconnecting.
    pub retry_at_ms: Option<u64>,
    /// The other people in the room.
    pub peers: usize,
    /// A standing problem the user should know about, if any.
    pub problem: Option<Problem>,
}

impl Status {
    pub(crate) fn idle() -> Self {
        Status {
            state: ConnState::Idle,
            stopped: None,
            text: "Not shared. Working on this device only.".to_owned(),
            unsynced: false,
            retry_at_ms: None,
            peers: 0,
            problem: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ErrorCode; 14] = [
        ErrorCode::Malformed,
        ErrorCode::UnsupportedVersion,
        ErrorCode::Unauthorized,
        ErrorCode::Forbidden,
        ErrorCode::RoomFull,
        ErrorCode::TooLarge,
        ErrorCode::RateLimited,
        ErrorCode::ResyncRequired,
        ErrorCode::Replaced,
        ErrorCode::NotJoined,
        ErrorCode::AlreadyJoined,
        ErrorCode::InvalidUpdate,
        ErrorCode::Unavailable,
        ErrorCode::Internal,
    ];

    #[test]
    fn every_error_code_has_a_sentence_and_an_action() {
        for code in ALL {
            for joining in [false, true] {
                let p = explain(code, joining);
                assert_eq!(p.code, Some(code));
                assert!(p.text.ends_with('.') && p.text.len() > 20, "{code:?}: {}", p.text);
                assert!(!p.text.contains("Error") && !p.text.contains("code"), "{code:?}: {}", p.text);
            }
        }
    }

    #[test]
    fn the_gate_has_three_texts_with_the_way_forward() {
        let required = explain_access(AccessDenial::Required, "canvas.example.com", true);
        assert_eq!(
            required.text,
            "This server is private. Open Preferences, Collaboration, and enter the access token for \
             canvas.example.com. The server's owner can create one."
        );
        assert_eq!((required.action, required.fatal), (ProblemAction::AskAdmin, true));
        let rejected = explain_access(AccessDenial::Rejected, "canvas.example.com", false);
        assert_eq!(
            rejected.text,
            "The server did not accept the access token. It may have expired or been revoked. Ask the server's owner \
             for a new one."
        );
        assert_eq!((rejected.action, rejected.fatal), (ProblemAction::AskAdmin, false));
        let expired = explain_access(AccessDenial::SignInExpired, "canvas.example.com", false);
        assert_eq!(
            expired.text,
            "Your sign-in has expired. Reload the page to sign in again. Your drawing is saved on this device."
        );
        assert_eq!(expired.action, ProblemAction::Reload);
        assert!(!rejected.text.contains("canvas.example.com"));
    }

    #[test]
    fn codes_the_protocol_says_not_to_retry_are_fatal() {
        for code in ALL {
            let p = explain(code, false);
            if matches!(
                code,
                ErrorCode::UnsupportedVersion | ErrorCode::Unauthorized | ErrorCode::Forbidden | ErrorCode::Replaced
            ) {
                assert!(p.fatal, "{code:?}");
                assert!(stop_reason(code).is_some());
            } else {
                assert!(!p.fatal, "{code:?}");
                assert!(stop_reason(code).is_none());
            }
            if code.retryable() {
                assert!(!p.fatal, "{code:?} is retryable");
            }
        }
        assert!(explain(ErrorCode::TooLarge, true).fatal);
    }
}
