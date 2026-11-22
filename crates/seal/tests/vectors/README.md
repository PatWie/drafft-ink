# Known-answer vectors

`vectors.json` holds the vectors of `tests/vectors.rs`: the subkey of each HKDF label, the membership token, the room
id, the link, a keyed image name, and one sealed record of each kind (update, an update with no payload, snapshot,
presence, direct, image) under a fixed room key and fixed nonces.

The standard vectors are in the test itself: XChaCha20-Poly1305 from draft-irtf-cfrg-xchacha-03 appendix A.3.1 and HKDF-SHA256
from RFC 5869 appendix A.1 to A.3.

## How they were checked

The vectors were not made by this crate. A Python script written from `docs/design/collab-e2e.md` sections 5 and 6, without
looking at the Rust code, produced them with libsodium (PyNaCl 1.6.2, `crypto_aead_xchacha20poly1305_ietf`), `cryptography`
50.0.2 (HKDF) and `hashlib`/`hmac`. The script:

1. checked libsodium against the draft-irtf-cfrg-xchacha-03 A.3.1 vector and `cryptography`'s HKDF against RFC 5869 A.1;
2. derived every subkey twice, with `cryptography` and with HKDF-Expand written out by hand (`HMAC(PRK, info || 0x01)`), and
   compared them;
3. built each record by hand (header, `AAD = "drafft.ink e2e v1" | room id | header [| image name]`, plaintext
   `len u32 LE | payload`), sealed it with libsodium and opened it again.

`tests/vectors.rs` then computes the same values with this crate, and with the same fixed nonces, and compares bytes.
It also opens each vector record. Native and `wasm32` runs use the same file, so both agree byte for byte.

To make the vectors again, redo the three steps above. Do not copy the Rust code: a copy would check nothing.

## Running them under wasm32

`cargo xtask wasm` checks and lints the crate for `wasm32-unknown-unknown`; it does not run tests. The test files of this crate
(except the million-seal test) also run as `wasm-bindgen-test` tests in Node, with the real `crypto.getRandomValues` behind
`getrandom`:

```sh
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  cargo test --target wasm32-unknown-unknown -p drafftink-seal
```

`wasm-bindgen-test-runner` must be the same version as `wasm-bindgen` (0.2.130), and Node must be 20 or newer: Node 18 aborts with a
V8 fatal error before the first test.
