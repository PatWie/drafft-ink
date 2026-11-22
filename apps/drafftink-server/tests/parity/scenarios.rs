//! The scenarios. Each one takes a running relay and runs the real clients against it. A scenario never asks which
//! target it runs on: whatever differs between targets is a value of `Relay::traits`.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use drafftink_collab::{ConnState, StopReason};
use drafftink_protocol::v2::{ErrorCode, JoinMode};
use drafftink_seal::{RoomKey, RoomKeys};

use super::common::canary;
use super::common::http;
use super::common::room_client::{Peer, USER, picture};
use super::raw::Raw;
use super::relay::Relay;
use super::scene::{Room, drawing, expect};

/// How long the clients of the `hibernate` scenario sit still. A Durable Object hibernates after about ten seconds
/// without a message; the keepalive is every twenty, so the wait covers both.
const IDLE: Duration = Duration::from_secs(16);

/// A new room is created by the first client and found by the second: both are online, see each other by name, and a
/// late joiner gets what was drawn before it came.
pub fn join(relay: &mut Relay) {
    let room = Room::new(relay);
    let mut a = room.client(1, JoinMode::Create);
    room.draw(&mut a, "seed-one", 10.0);
    room.draw(&mut a, "seed-two", 20.0);
    room.settle(&mut [&mut a]);
    assert!(a.online() && a.session.client().status().state == ConnState::Online);

    let mut b = room.client(2, JoinMode::Open);
    room.settle(&mut [&mut a, &mut b]);
    assert_eq!(drawing(&b), expect(&[("seed-one", 10), ("seed-two", 20)]), "the second client got what was drawn");
    room.until(&mut [&mut a, &mut b], "both to see the other by name", |p| {
        p.iter().all(|c| {
            let peers = c.session.client().peers();
            peers.len() == 1 && peers.iter().all(|other| other.user.as_ref().is_some_and(|u| u.name == USER))
        })
    });
    assert!(a.session.client().sealed().is_some_and(|c| c.expires_at_ms() > 0), "the room announces its deadline");
    assert!(a.session.client().room_limits().is_some(), "the room announces its limits");

    room.draw(&mut b, "later", 30.0);
    room.settle(&mut [&mut a, &mut b]);
    let mut c = room.client(3, JoinMode::Open);
    room.settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(drawing(&c), expect(&[("seed-one", 10), ("seed-two", 20), ("later", 30)]), "a late joiner catches up");
}

/// Edits flow both ways: adding, moving and deleting, and a cursor is seen by the other client.
pub fn edit(relay: &mut Relay) {
    let room = Room::new(relay);
    let mut a = room.client(1, JoinMode::Create);
    let mut b = room.client(2, JoinMode::Create);
    room.settle(&mut [&mut a, &mut b]);

    room.draw(&mut a, "from-a", 10.0);
    room.until(&mut [&mut a, &mut b], "b to see a's rectangle", |p| drawing(p[1]) == expect(&[("from-a", 10)]));
    room.draw(&mut b, "from-b", 20.0);
    room.until(&mut [&mut a, &mut b], "a to see b's rectangle", |p| drawing(p[0]).len() == 2);
    room.move_to(&mut a, "from-b", 25.0);
    room.erase(&mut b, "from-a");
    let want = expect(&[("from-b", 25)]);
    room.until(&mut [&mut a, &mut b], "both to agree after a move and a delete", |p| {
        drawing(p[0]) == want && drawing(p[1]) == want
    });
    room.settle(&mut [&mut a, &mut b]);

    let at = drafftink_protocol::Point { x: 40.0, y: 50.0 };
    a.session.client_mut().set_cursor(room.clock.now(), Some(at));
    room.until(&mut [&mut a, &mut b], "b to see a's cursor", |p| {
        p[1].session.client().peers().iter().any(|other| other.cursor == Some(at))
    });
}

/// A client that loses its network keeps editing; when the network is back both sides have everything.
pub fn offline(relay: &mut Relay) {
    let room = Room::new(relay);
    let mut a = room.client(1, JoinMode::Create);
    let mut b = room.client(2, JoinMode::Create);
    room.draw(&mut a, "before", 10.0);
    room.settle(&mut [&mut a, &mut b]);
    room.until(&mut [&mut a, &mut b], "b to have the first rectangle", |p| drawing(p[1]).len() == 1);

    room.taps.cut(2);
    room.until(&mut [&mut a, &mut b], "b to notice that it is offline", |p| !p[1].online());
    room.draw(&mut b, "offline-b", 20.0);
    room.draw(&mut a, "meanwhile-a", 30.0);
    room.until(&mut [&mut a, &mut b], "a's edit to be stored while b is away", |p| p[0].settled());
    assert!(b.session.client().status().unsynced, "b knows that its edit is not shared yet");
    assert_eq!(drawing(&b), expect(&[("before", 10), ("offline-b", 20)]), "b does not have a's edit yet");

    room.taps.restore(2);
    let want = expect(&[("before", 10), ("offline-b", 20), ("meanwhile-a", 30)]);
    room.until(&mut [&mut a, &mut b], "both to have all three rectangles", |p| {
        drawing(p[0]) == want && drawing(p[1]) == want && p.iter().all(|c| c.settled())
    });
    assert!(b.session.client().stats().connects >= 2, "b connected again by itself");
}

/// Pictures travel as sealed files: offered by one client, wanted by others, equal byte for byte, one stored name for
/// the same picture twice, and a late joiner gets them too.
pub fn images(relay: &mut Relay) {
    let room = Room::new(relay);
    let mut a = room.client(1, JoinMode::Create);
    let mut b = room.client(2, JoinMode::Create);
    room.settle(&mut [&mut a, &mut b]);

    let first = picture("images-scenario", 1, 3_000);
    let second = picture("images-scenario", 2, 5_000);
    let (id1, held1) = a.put_picture(room.clock, "image/png", &first);
    let (id2, held2) = a.put_picture(room.clock, "image/png", &second);
    a.offer(room.clock, &id1, &held1);
    a.offer(room.clock, &id2, &held2);
    room.until(&mut [&mut a, &mut b], "b to know both pictures", |p| {
        p[1].store.manifest(&id1).is_some() && p[1].store.manifest(&id2).is_some()
    });
    b.want(room.clock, &id1);
    b.want(room.clock, &id2);
    room.until(&mut [&mut a, &mut b], "b to download both", |p| {
        p[1].arrived(&id1).is_some() && p[1].arrived(&id2).is_some()
    });
    assert_eq!(b.arrived(&id1).as_deref(), Some(first.as_slice()), "the first picture arrived unchanged");
    assert_eq!(b.arrived(&id2).as_deref(), Some(second.as_slice()), "the second picture arrived unchanged");

    // The same picture again is the same file: nothing new is stored.
    let (again, held_again) = a.put_picture(room.clock, "image/png", &first);
    assert_eq!(again, id1, "the same bytes are the same file id");
    a.offer(room.clock, &again, &held_again);
    room.settle(&mut [&mut a, &mut b]);
    let names: HashSet<String> = room
        .taps
        .exchanges()
        .iter()
        .filter(|e| e.method == "PUT" && e.status.is_some_and(|s| s < 300))
        .map(|e| e.url.clone())
        .collect();
    assert_eq!(names.len(), 2, "two pictures are stored under two names: {names:?}");
    assert_eq!(a.session.client().images().room_images, 2, "the room counts two pictures");

    let mut c = room.client(3, JoinMode::Open);
    room.until(&mut [&mut a, &mut b, &mut c], "c to know both pictures", |p| {
        p[2].store.manifest(&id1).is_some() && p[2].store.manifest(&id2).is_some()
    });
    c.want(room.clock, &id1);
    c.want(room.clock, &id2);
    room.until(&mut [&mut a, &mut b, &mut c], "c to download both", |p| {
        p[2].arrived(&id1).is_some() && p[2].arrived(&id2).is_some()
    });
    assert_eq!(c.arrived(&id2).as_deref(), Some(second.as_slice()), "a late joiner gets the pictures");
    for who in [&a, &b, &c] {
        assert!(!who.damaged(&id1) && !who.damaged(&id2), "no client saw a damaged picture");
    }
}

/// A room that nobody visits is deleted; an `open` join afterwards is `RoomMissing`, and a `create` join makes a new,
/// empty room under the same key.
pub fn expire(relay: &mut Relay) {
    let (ttl, grace, slack) = (relay.life.ttl(), relay.life.grace(), relay.traits.deletion_slack);
    let room = Room::new(relay);
    let visited = Instant::now();
    let mut a = room.client(1, JoinMode::Create);
    room.draw(&mut a, "gone-soon", 10.0);
    room.settle(&mut [&mut a]);
    let left_at = Instant::now();
    a.session.client_mut().leave(room.clock.now(), &mut a.store);
    room.idle(&mut [&mut a], Duration::from_millis(500));
    drop(a);

    // Not early: while the deadline is far, the room is there. Listing the pictures is no visit, so it moves nothing.
    // (On a machine too slow to get here in time the check is left out and says so.)
    let list = format!("/r/{}/f", room.keys.room_id());
    let member = [("Authorization", member_header(&room.keys))];
    if visited.elapsed() + Duration::from_secs(1) < ttl {
        let status = http::request(relay.addr, "GET", &list, &member, &[]).status;
        assert_eq!(status, 200, "the room is not deleted before its deadline");
    } else {
        eprintln!("expire: the machine is too slow to look at the room before its deadline; that check is skipped");
    }

    // No probe in between: every join is a visit and would move the deadline.
    let until = left_at + ttl + grace + slack;
    std::thread::sleep(until.saturating_duration_since(Instant::now()));

    let mut opener = room.client(2, JoinMode::Open);
    room.until(&mut [&mut opener], "the relay to answer the open join", |p| {
        p[0].session.client().status().stopped.is_some() || p[0].online()
    });
    assert_eq!(
        opener.session.client().status().stopped,
        Some(StopReason::RoomMissing),
        "the room was deleted after its deadline: {:?}",
        opener.session.client().status()
    );

    assert_eq!(
        http::request(relay.addr, "GET", &list, &member, &[]).status,
        404,
        "the picture route finds no room either"
    );

    let mut maker = room.client(3, JoinMode::Create);
    room.settle(&mut [&mut maker]);
    assert!(drawing(&maker).is_empty(), "the new room is empty: nothing of the old one is left");
    let mut visitor = room.client(4, JoinMode::Open);
    room.settle(&mut [&mut maker, &mut visitor]);
    assert!(visitor.online(), "once made, the room can be opened again");
}

/// The wrong token is refused with `Unauthorized` on the socket and with 401 on the picture routes; a room nobody
/// made is `RoomMissing`; and none of it disturbs the members.
pub fn membership(relay: &mut Relay) {
    let room = Room::new(relay);
    let mut a = room.client(1, JoinMode::Create);
    room.draw(&mut a, "mine", 10.0);
    room.settle(&mut [&mut a]);

    let stranger = RoomKeys::new(&RoomKey::generate().expect("a random key"));
    for mode in [JoinMode::Open, JoinMode::Create] {
        let mut raw = Raw::connect(relay, room.keys.room_id(), 9);
        raw.join(room.keys.room_id(), &stranger, mode);
        assert_eq!(raw.error(), Some(ErrorCode::Unauthorized), "a wrong token is refused ({mode:?})");
    }

    let name = room.keys.image_name(&[3; 32]);
    let path = format!("/r/{}/f/{name}", room.keys.room_id());
    let sealed = room.keys.seal_image(&name, &picture("membership", 1, 300)).expect("seals").into_bytes();
    let theirs = ("Authorization", member_header(&stranger));
    for (what, reply) in [
        ("a stranger's PUT", http::request(relay.addr, "PUT", &path, std::slice::from_ref(&theirs), &sealed)),
        ("a stranger's GET", http::request(relay.addr, "GET", &path, std::slice::from_ref(&theirs), &[])),
        ("a PUT without a token", http::request(relay.addr, "PUT", &path, &[], &sealed)),
        ("a GET without a token", http::request(relay.addr, "GET", &path, &[], &[])),
    ] {
        assert_eq!(reply.status, 401, "{what} is refused: {}", String::from_utf8_lossy(&reply.body));
    }
    let list = format!("/r/{}/f", room.keys.room_id());
    assert_eq!(http::request(relay.addr, "GET", &list, std::slice::from_ref(&theirs), &[]).status, 401);
    let mine = ("Authorization", member_header(&room.keys));
    assert_eq!(
        http::request(relay.addr, "GET", &list, std::slice::from_ref(&mine), &[]).status,
        200,
        "a member may list"
    );
    assert_eq!(
        http::request(relay.addr, "GET", &path, std::slice::from_ref(&mine), &[]).status,
        404,
        "a member asks for a picture that was never stored"
    );

    let nobody = Room::new(relay);
    let mut lost = nobody.client(5, JoinMode::Open);
    nobody.until(&mut [&mut lost], "the relay to answer the open join", |p| {
        p[0].session.client().status().stopped.is_some()
    });
    assert_eq!(lost.session.client().status().stopped, Some(StopReason::RoomMissing), "an open join makes no room");
    let mut check = nobody.client(6, JoinMode::Open);
    nobody.until(&mut [&mut check], "the relay to answer again", |p| p[0].session.client().status().stopped.is_some());
    assert_eq!(check.session.client().status().stopped, Some(StopReason::RoomMissing), "and it still made none");

    room.draw(&mut a, "still-fine", 20.0);
    room.settle(&mut [&mut a]);
    let mut b = room.client(2, JoinMode::Open);
    room.settle(&mut [&mut a, &mut b]);
    assert_eq!(drawing(&b), expect(&[("mine", 10), ("still-fine", 20)]), "the members are not disturbed");
}

/// Clients sit still for longer than a Durable Object stays awake. A client that is alone in its room has nothing to
/// say, so its keepalive is what the relay hears: it is answered and the connection stays open. A second client then
/// joins, both sit still again (two clients send presence heartbeats to each other), and an edit made after each nap
/// still reaches the other client.
pub fn hibernate(relay: &mut Relay) {
    let room = Room::new(relay);
    let mut a = room.client(1, JoinMode::Create);
    room.settle(&mut [&mut a]);

    let alone = Instant::now();
    room.until(&mut [&mut a], "a keepalive answer for the client that is alone", |p| {
        alone.elapsed() > IDLE && room.taps.pongs(1) >= 1 && p[0].online()
    });
    assert_eq!(connections(&a), (1, 0), "the connection was never lost while it was alone");

    let mut b = room.client(2, JoinMode::Open);
    room.settle(&mut [&mut a, &mut b]);
    room.draw(&mut a, "after-the-first-nap", 10.0);
    room.until(&mut [&mut a, &mut b], "b to see an edit made after the first nap", |p| {
        drawing(p[1]) == expect(&[("after-the-first-nap", 10)])
    });

    let together = Instant::now();
    room.until(&mut [&mut a, &mut b], "both clients to sit still", |_| together.elapsed() > IDLE);
    for who in [&a, &b] {
        assert!(who.online(), "the connection is still open");
    }
    assert_eq!(connections(&a), (1, 0), "a was not disconnected");
    assert_eq!(connections(&b), (1, 0), "b was not disconnected");
    room.draw(&mut b, "after-the-second-nap", 20.0);
    room.until(&mut [&mut a, &mut b], "a to see an edit made after the second nap", |p| drawing(p[0]).len() == 2);
    room.settle(&mut [&mut a, &mut b]);
}

/// How often a client connected and reconnected.
fn connections(who: &Peer) -> (u32, u32) {
    let stats = who.session.client().stats();
    (stats.connects, stats.reconnects)
}

/// The relay is blind: the canary session of `blind.rs` against this target, then a search of everything the relay
/// stored. The search is shown to be able to fail on this target's files.
pub fn canary(relay: &mut Relay) {
    // The canary has a fixed key and the relay of this test is new, so the room is free.
    let session = canary::run_session(&relay.url, relay.addr);
    relay.stop();
    canary::assert_blind_relay(&session, relay.stored());
    canary::assert_search_can_fail(&session, relay.stored());
}

fn member_header(keys: &RoomKeys) -> String {
    use base64ct::{Base64UrlUnpadded, Encoding};
    format!("Drafft-Member {}", Base64UrlUnpadded::encode_string(keys.membership_token().expose_bytes()))
}
