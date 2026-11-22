// The bindings that `env` of the test runtime has: those of the Worker.
declare namespace Cloudflare {
  interface Env {
    ROOM: DurableObjectNamespace<import("../src/room").Room>;
    ASSETS: Fetcher;
  }
  interface GlobalProps {
    mainModule: typeof import("./worker");
  }
}
