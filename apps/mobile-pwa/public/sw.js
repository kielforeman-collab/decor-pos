// Hand-rolled service worker for the Decor POS mobile PWA.
// No build step, no new dependencies: this file is copied verbatim
// into dist/ by Vite (public/) and registered from main.tsx.
//
// Strategy:
// - App shell (/, /index.html, /assets/*, manifest, icons):
//   cache-first, populated on install. The app stays usable offline.
// - API calls (/api/*): network-only, never cached — stale stock,
//   dispatch or KPI data would be worse than an error, and every
//   mutation goes through the outbox (src/offline.ts) instead.
// - Anything else: network-first, fall back to cache, then offline.

const CACHE = 'decor-pos-mobile-v1'
const SHELL = ['/', '/index.html', '/manifest.webmanifest', '/icon-192.png', '/icon-512.png']

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  )
})

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  )
})

self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url)
  if (url.pathname.startsWith('/api/')) return // network-only: never cache API
  if (event.request.method !== 'GET') return

  const isShell =
    url.pathname === '/' ||
    url.pathname === '/index.html' ||
    url.pathname.startsWith('/assets/') ||
    url.pathname === '/manifest.webmanifest' ||
    url.pathname.startsWith('/icon-')

  event.respondWith(
    (async () => {
      const cache = await caches.open(CACHE)
      if (isShell) {
        const hit = await cache.match(event.request)
        if (hit) return hit
      }
      try {
        const res = await fetch(event.request)
        if (res.ok && isShell) void cache.put(event.request, res.clone())
        return res
      } catch {
        const hit = await cache.match(event.request)
        if (hit) return hit
        if (event.request.mode === 'navigate') {
          const index = await cache.match('/index.html')
          if (index) return index
        }
        throw new Error('offline and not cached')
      }
    })(),
  )
})
