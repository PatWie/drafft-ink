//! What the tests of the commands share to put pictures into an encrypted room: sessions that talk to a fake relay
//! (`collab::fake_relay`), and pictures of exact sizes.

use drafftink_doc::FilePolicy;
use drafftink_relay::RelayConfig;
use drafftink_scene::FileId;
use serde_json::{Value, json};

use crate::cmd::collab::fake_relay::FakeRelay;
use crate::ports::fake::Fakes;
use crate::ports::{Clock, Ns};
use crate::session::Session;
use crate::testing;

/// The address the fake network serves.
const URL: &str = "ws://127.0.0.1:3030/ws";

/// Sessions on one fake relay.
pub(crate) struct Room {
    relay: FakeRelay,
    /// The sessions with their fakes.
    pub c: Vec<(Session, Fakes)>,
}

impl Room {
    /// `n` fresh sessions.
    pub(crate) fn new(n: usize) -> Self {
        let c = (0..n)
            .map(|i| {
                let (mut s, fakes) = testing::session();
                // Distinct element ids per session, as two people would have.
                for _ in 0..i * 100 {
                    s.ports.ids.element_id();
                }
                (s, fakes)
            })
            .collect();
        Self::of(c)
    }

    /// The given sessions.
    pub(crate) fn of(c: Vec<(Session, Fakes)>) -> Self {
        // The relay answers no picture request: a session that joins holds the manifests and not the bytes, which is
        // what these tests are about. The glue that moves pictures has its own tests (`collab_pictures_tests`).
        let mut relay = FakeRelay::new(RelayConfig::self_host());
        relay.hold_blobs(true);
        Self { relay, c }
    }

    /// The session `i`.
    pub(crate) fn s(&mut self, i: usize) -> &mut Session {
        &mut self.c[i].0
    }

    /// The fakes of session `i`.
    pub(crate) fn fakes(&self, i: usize) -> &Fakes {
        &self.c[i].1
    }

    /// Lets the sessions and the relay talk for a while.
    pub(crate) fn settle(&mut self) {
        for _ in 0..80 {
            for (s, f) in &mut self.c {
                f.clock.advance_ms(40);
                s.collab_poll();
                self.relay.attach(&f.net);
            }
            let now = self.c.first().map_or(0, |(_, f)| f.clock.mono_ms());
            self.relay.step(now);
        }
    }

    /// Session `i` makes a new encrypted room, and returns its link. The pictures it stores stay out of the
    /// document's bytes, as in a real room (`FilePolicy::External`).
    pub(crate) fn create(&mut self, i: usize) -> String {
        run(self.s(i), "collab.connect", json!({ "url": URL }));
        run(self.s(i), "collab.join_room", json!({}));
        self.settle();
        self.external(i);
        let shared = run(self.s(i), "collab.share_link", json!({ "copy": false, "reveal": true }));
        shared["link"].as_str().map(str::to_owned).unwrap_or_default()
    }

    /// Session `i` joins the room of `link`.
    pub(crate) fn join(&mut self, i: usize, link: &str) {
        run(self.s(i), "collab.connect", json!({ "url": URL }));
        run(self.s(i), "collab.join_room", json!({ "room": link, "on_local": "merge" }));
        self.settle();
        self.external(i);
    }

    /// New pictures of session `i` go into its document as `{mime, size}` only.
    pub(crate) fn external(&mut self, i: usize) {
        self.s(i).tab_mut().doc.replicated_mut().set_file_policy(FilePolicy::External);
    }

    /// Session `i` writes the bytes of a picture to its blob cache, as a download or an insert does.
    pub(crate) fn cache(&mut self, i: usize, bytes: &[u8]) -> FileId {
        let id = FileId::of(bytes);
        let mut storage = self.c[i].1.storage.clone();
        if let Err(e) = crate::ports::Storage::put(&mut storage, Ns::Blobs, id.as_str(), bytes) {
            panic!("the blob cache refused a picture: {e}");
        }
        id
    }

    /// Takes session `i` off the network: the relay stops answering, so the client gives up and cannot reconnect.
    /// Its tab stays bound to the room.
    pub(crate) fn go_offline(&mut self, i: usize) {
        self.c[i].1.net.set_fail_open(true);
        for _ in 0..600 {
            self.c[i].1.clock.advance_ms(1_000);
            self.c[i].0.collab_poll();
        }
    }
}

fn run(s: &mut Session, id: &str, args: Value) -> Value {
    s.execute(id, args).unwrap_or_else(|r| panic!("{id} was refused: {} ({})", r.message, r.code))
}

/// A valid PNG of exactly `total` bytes: a 1x1 picture with one ancillary chunk that holds the padding, whose
/// bytes follow from `seed`, so pictures with different seeds are different pictures.
pub(crate) fn png_of_size(total: usize, seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 1, 1);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("a PNG header");
        writer.write_image_data(&[seed]).expect("PNG pixels");
    }
    // The last 12 bytes are the IEND chunk. A chunk costs 12 bytes of framing.
    let iend = out.split_off(out.len() - 12);
    let body = total.checked_sub(out.len() + iend.len() + 12).expect("a size large enough for a PNG");
    let mut data = Vec::with_capacity(body);
    let mut x = u32::from(seed) | 0x9E37_0000;
    for _ in 0..body {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        data.push((x >> 8) as u8);
    }
    let kind = *b"prVt";
    out.extend_from_slice(&u32::try_from(body).expect("a chunk that fits").to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(&data);
    out.extend_from_slice(&crc32(&[&kind, &data]).to_be_bytes());
    out.extend_from_slice(&iend);
    assert_eq!(out.len(), total);
    out
}

/// CRC-32 (IEEE) of the parts one after the other, as PNG chunks use it.
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = !0u32;
    for byte in parts.iter().flat_map(|p| p.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_padded_png_has_the_size_asked_for_and_is_a_picture() {
        let bytes = png_of_size(5_000, 3);
        assert_eq!(bytes.len(), 5_000);
        let probed = drafftink_io::picture::probe(&bytes).expect("a picture");
        assert_eq!((probed.width, probed.height), (1, 1));
        assert_ne!(FileId::of(&bytes), FileId::of(&png_of_size(5_000, 4)));
    }
}

#[cfg(test)]
mod offline_tests {
    use super::*;

    #[test]
    fn a_session_that_lost_the_relay_is_offline_and_keeps_its_room() {
        let mut w = Room::new(1);
        w.create(0);
        assert!(w.s(0).room_images().is_some_and(|r| r.online));
        w.go_offline(0);
        assert!(w.s(0).room_images().is_some_and(|r| !r.online));
    }
}
