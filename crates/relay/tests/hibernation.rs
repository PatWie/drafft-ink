#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A Durable Object hibernates and the host rebuilds the core over the same stores while the sockets stay open.

mod common;

use common::*;
use drafftink_relay::ConnId;

#[test]
fn member_of_names_a_joined_connection_and_nobody_else() {
    let mut h = Harness::self_host();
    let waiting = h.connect();
    let (a, b) = (h.open(1), h.open(2));
    let (id_a, order_a) = h.relay.member_of(a).unwrap();
    let (id_b, order_b) = h.relay.member_of(b).unwrap();
    assert_eq!((id_a, id_b), (client(1), client(2)));
    assert!(order_a < order_b, "the longest-connected person has the smallest order");
    assert_eq!(h.relay.member_of(waiting), None, "not joined");
    assert_eq!(h.relay.member_of(ConnId(999)), None, "unknown");
    h.close(a);
    assert_eq!(h.relay.member_of(a), None, "gone");
}
