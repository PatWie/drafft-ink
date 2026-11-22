// The bindings that `env` of the test runtime has: those of the Worker, and what the test config adds.
declare namespace Cloudflare {
  interface Env {
    ROOM: DurableObjectNamespace<import("../src/room").Room>;
    ASSETS: Fetcher;
    BLOBS: R2Bucket;
    /** The private key that signs the Access tokens of the private tests (the Worker is given its public half). */
    TEST_ACCESS_PRIVATE_JWK: string;
    TEST_ACCESS_KEY_ID: string;
  }
  interface GlobalProps {
    mainModule: typeof import("./worker");
  }
}
