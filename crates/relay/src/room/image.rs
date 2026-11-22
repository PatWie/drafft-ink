//! The image routes: `PUT`, `GET` and the list of the sealed pictures of one room (design section 9.3).
//!
//! Images do not move the retention deadline, and a request for a room that does not exist creates nothing.

use drafftink_protocol::SEAL_OVERHEAD;
use drafftink_protocol::v2::{ImageName, RecordHeader, RecordKind};

use super::{Output, Relay, StoreOp};
use crate::images::{ImageBody, ImageOp, ImageRequest, ImageResponse, ImageStatus};
use crate::store::{BlobStore, RelayStore};

impl<S: RelayStore, B: BlobStore> Relay<S, B> {
    /// Answers an image request. `now_ms` is only used for the upload rate limit.
    pub fn image_request(&mut self, now_ms: u64, request: ImageRequest<'_>) -> (ImageResponse, Output) {
        let mut out = Output::default();
        let response = self.serve_image(now_ms, request, &mut out);
        (response, self.finish(now_ms, out))
    }

    fn serve_image(&mut self, now_ms: u64, request: ImageRequest<'_>, out: &mut Output) -> ImageResponse {
        if !self.room.accepts(request.proof) {
            return ImageResponse::status(ImageStatus::Unauthorized);
        }
        match self.load_meta() {
            Ok(Some(_)) => {}
            Ok(None) => return ImageResponse::status(ImageStatus::RoomMissing),
            Err(_) => return ImageResponse::status(ImageStatus::Unavailable),
        }
        match request.op {
            ImageOp::Put { name, body } => self.put_image(now_ms, &name, body, out),
            ImageOp::Get { name } => match self.blobs.get(&name) {
                Ok(Some(sealed)) => ImageResponse::with(ImageStatus::Ok, ImageBody::Sealed(sealed)),
                Ok(None) => ImageResponse::status(ImageStatus::ImageMissing),
                Err(_) => ImageResponse::status(ImageStatus::Unavailable),
            },
            ImageOp::List => match self.blobs.list() {
                Ok(list) => ImageResponse::with(ImageStatus::Ok, ImageBody::List(list)),
                Err(_) => ImageResponse::status(ImageStatus::Unavailable),
            },
        }
    }

    fn put_image(&mut self, now_ms: u64, name: &ImageName, body: &[u8], out: &mut Output) -> ImageResponse {
        let limits = &self.cfg.limits;
        if body.len() > (limits.max_image_bytes as usize).saturating_add(SEAL_OVERHEAD) {
            return ImageResponse::status(ImageStatus::TooLarge);
        }
        if RecordHeader::peek(body).map_or(true, |h| h.kind != RecordKind::Image) {
            return ImageResponse::status(ImageStatus::BadRequest);
        }
        if let Err(wait) = self.image_puts.take(now_ms) {
            return ImageResponse {
                status: ImageStatus::RateLimited,
                retry_after_ms: Some(wait),
                body: ImageBody::Empty,
            };
        }
        let (stats, existing) = match (self.blobs.stats(), self.blobs.head(name)) {
            (Ok(stats), Ok(existing)) => (stats, existing),
            _ => return ImageResponse::status(ImageStatus::Unavailable),
        };
        // Replacing an image is accounted as the difference, so a damaged name can be healed in a full room.
        let bytes_after = stats.bytes.saturating_sub(existing.unwrap_or(0)).saturating_add(body.len() as u64);
        let count_after = stats.count.saturating_add(u32::from(existing.is_none()));
        let limits = &self.cfg.limits;
        if bytes_after > limits.max_room_image_bytes || count_after > limits.max_room_images {
            return ImageResponse::status(ImageStatus::RoomFull);
        }
        if self.blobs.put(name, body).is_err() {
            return ImageResponse::status(ImageStatus::Unavailable);
        }
        out.store_ops.push(StoreOp::PutImage { bytes: body.len() as u64 });
        let totals = crate::store::BlobStats { count: count_after, bytes: bytes_after };
        ImageResponse::with(ImageStatus::Created, ImageBody::Totals(totals))
    }
}
