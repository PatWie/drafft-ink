//! The image routes of a relay (design section 9.3), as plain data. The host maps them to HTTP: `PUT`, `GET` and
//! `GET` of the list under `/r/<room>/f/<name>`, with the membership token in the `Authorization` header.

use drafftink_protocol::v2::{ImageName, Proof};

use crate::store::{BlobInfo, BlobStats};

/// What a request asks for.
#[derive(Debug, Clone, Copy)]
pub enum ImageOp<'a> {
    /// Stores the sealed image under `name`. The body must be a sealed record of kind `Image`.
    Put {
        /// The keyed name.
        name: ImageName,
        /// The sealed image.
        body: &'a [u8],
    },
    /// Reads the sealed image under `name`.
    Get {
        /// The keyed name.
        name: ImageName,
    },
    /// Lists the room's images by name and size.
    List,
}

/// A request for the images of one room.
#[derive(Debug, Clone, Copy)]
pub struct ImageRequest<'a> {
    /// The membership proof from the `Authorization` header.
    pub proof: &'a Proof,
    /// What to do.
    pub op: ImageOp<'a>,
}

/// How a request ended. The numbers are the HTTP statuses of the design; [`code`](Self::code) is the stable word in
/// the JSON body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageStatus {
    /// The image is stored (201).
    Created,
    /// The read succeeded (200).
    Ok,
    /// The body is not a sealed image (400).
    BadRequest,
    /// The proof does not belong to the room (401).
    Unauthorized,
    /// The room does not exist (404).
    RoomMissing,
    /// The room has no image of that name (404).
    ImageMissing,
    /// The sealed image is larger than the limit (413).
    TooLarge,
    /// The room holds as many images or bytes as it may (507).
    RoomFull,
    /// Too many uploads (429).
    RateLimited,
    /// The store failed (503).
    Unavailable,
}

impl ImageStatus {
    /// The HTTP status.
    pub const fn http(self) -> u16 {
        match self {
            ImageStatus::Created => 201,
            ImageStatus::Ok => 200,
            ImageStatus::BadRequest => 400,
            ImageStatus::Unauthorized => 401,
            ImageStatus::RoomMissing | ImageStatus::ImageMissing => 404,
            ImageStatus::TooLarge => 413,
            ImageStatus::RoomFull => 507,
            ImageStatus::RateLimited => 429,
            ImageStatus::Unavailable => 503,
        }
    }

    /// A stable word for the JSON body of a failure.
    pub const fn code(self) -> &'static str {
        match self {
            ImageStatus::Created => "created",
            ImageStatus::Ok => "ok",
            ImageStatus::BadRequest => "bad_request",
            ImageStatus::Unauthorized => "unauthorized",
            ImageStatus::RoomMissing => "room_missing",
            ImageStatus::ImageMissing => "image_missing",
            ImageStatus::TooLarge => "image_too_large",
            ImageStatus::RoomFull => "room_images_full",
            ImageStatus::RateLimited => "rate_limited",
            ImageStatus::Unavailable => "unavailable",
        }
    }
}

/// What a response carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageBody {
    /// Nothing.
    Empty,
    /// The sealed image.
    Sealed(Vec<u8>),
    /// The room's images.
    List(Vec<BlobInfo>),
    /// The room's totals after a `PUT`.
    Totals(BlobStats),
}

/// The answer to an [`ImageRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageResponse {
    /// How the request ended.
    pub status: ImageStatus,
    /// For [`ImageStatus::RateLimited`]: when to try again.
    pub retry_after_ms: Option<u32>,
    /// What it carries.
    pub body: ImageBody,
}

impl ImageResponse {
    pub(crate) fn status(status: ImageStatus) -> Self {
        ImageResponse { status, retry_after_ms: None, body: ImageBody::Empty }
    }

    pub(crate) fn with(status: ImageStatus, body: ImageBody) -> Self {
        ImageResponse { status, retry_after_ms: None, body }
    }
}
