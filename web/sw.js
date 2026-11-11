// Service worker of Drafft.ink: the app works offline after the first load.
//
// Rules (docs/web-app.md):
//  * The shell (page, script, wasm, icons) is stored when the worker installs, under a cache named after the
//    build. A new build is a new cache; the old one is deleted when the new worker activates.
//  * The page itself (navigations) goes network first, so a deployment shows up at once, and falls back to the
//    stored page when the network is gone.
//  * Everything else of ours is cache first. The script and the wasm carry ?v=<build> in their address, so a
//    cached file always belongs to the build that asked for it.
//  * /ws and /health, other origins and non-GET requests are never touched.
const BUILD = "__BUILD__";
const CACHE = `drafftink-${BUILD}`;
const SHELL = [
  "./",
  "index.html",
  "manifest.webmanifest",
  "favicon.svg",
  "icon-192.png",
  "icon-512.png",
  "icon-maskable-512.png",
  `pkg/drafftink_web.js?v=${BUILD}`,
  `pkg/drafftink_web_bg.wasm?v=${BUILD}`,
  // The font of the first frame. The other fonts are stored when the app first asks for them.
  ...__SHELL_FONTS__,
];
const NEVER = new Set(["/ws", "/health"]);

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(CACHE)
      // `reload` skips the browser's HTTP cache, so the stored copy is the server's current one.
      .then((cache) => cache.addAll(SHELL.map((url) => new Request(url, { cache: "reload" }))))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys()
      .then((names) => Promise.all(names.filter((n) => n.startsWith("drafftink-") && n !== CACHE).map((n) => caches.delete(n))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method !== "GET" || url.origin !== self.location.origin || NEVER.has(url.pathname)) return;

  if (request.mode === "navigate") {
    event.respondWith(
      fetch(request, { cache: "no-cache" }).catch(() =>
        caches.open(CACHE).then((cache) => cache.match("index.html")).then((hit) => hit || Response.error()),
      ),
    );
    return;
  }

  event.respondWith(
    caches.open(CACHE).then(async (cache) => {
      const hit = await cache.match(request);
      if (hit) return hit;
      const response = await fetch(request);
      // Keep what the page asked for later (fonts, a sample) only when it is a plain success.
      if (response.ok && response.status === 200) cache.put(request, response.clone()).catch(() => {});
      return response;
    }),
  );
});
