// sepia service worker — deliberately small. Only the content-hashed
// /pkg/ bundle is cache-first; every other same-origin asset is
// network-first with a cache fallback for offline, so a deploy can
// never pair fresh HTML with a stale fixed-name asset (style.css).
const CACHE = "sepia-v5";
const ASSETS = ["/", "/style.css", "/manifest.json", "/icon.svg"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(ASSETS))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || url.origin !== location.origin) return;

  // Never cache API / server-function traffic.
  if (url.pathname.startsWith("/api/") || url.pathname.startsWith("/hub/")) return;

  // Navigations: network first, fall back to the cached shell offline.
  if (event.request.mode === "navigate") {
    event.respondWith(
      fetch(event.request)
        .then((res) => {
          const copy = res.clone();
          void caches.open(CACHE).then((cache) => cache.put("/", copy));
          return res;
        })
        .catch(() => caches.match("/")),
    );
    return;
  }

  // The bundle stem is content-hashed (`sepia_web_<hash>.*`) — a
  // cached hit is byte-identical, so pkg/ is cache-first immutable.
  if (url.pathname.startsWith("/pkg/")) {
    event.respondWith(
      caches.match(event.request).then(
        (hit) =>
          hit ||
          fetch(event.request).then((res) => {
            if (res.ok) {
              const copy = res.clone();
              void caches.open(CACHE).then((cache) => cache.put(event.request, copy));
            }
            return res;
          }),
      ),
    );
    return;
  }

  // Other static assets (style.css, icon, manifest): network-first,
  // cached copy as the offline fallback. Fixed filenames mean a
  // cache-first hit could serve a stylesheet that predates the markup.
  event.respondWith(
    fetch(event.request)
      .then((res) => {
        if (res.ok) {
          const copy = res.clone();
          void caches.open(CACHE).then((cache) => cache.put(event.request, copy));
        }
        return res;
      })
      .catch(() => caches.match(event.request)),
  );
});

// Push notifications — the node sends {title, body, url, tag} JSON
// payloads (`url` is e.g. `/?session=<id>`); clicking focuses an open
// window or opens the link.
self.addEventListener("push", (event) => {
  const data = event.data ? event.data.json() : {};
  const options = {
    body: data.body || "",
    icon: "/icon.svg",
    tag: data.tag,
    data: { url: data.url || "/" },
  };
  event.waitUntil(self.registration.showNotification(data.title || "sepia", options));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = (event.notification.data && event.notification.data.url) || "/";
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((clients) => {
      const existing = clients.find((c) => c.url.startsWith(location.origin));
      if (existing) {
        void existing.focus();
        return existing.navigate(url);
      }
      return self.clients.openWindow(url);
    }),
  );
});
