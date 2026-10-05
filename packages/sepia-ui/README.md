# sepia-ui

The Sepia web client (`apps/web`) as a standalone static bundle — for
self-hosters running the UI apart from a `sepia-node` server. The package
ships `dist/` only; there is no server inside.

## Use it

```bash
npm i sepia-ui
npx serve node_modules/sepia-ui/dist     # or any static host / CDN / proxy
```

Two ways to attach it to a node:

- **Same origin (recommended)** — point the node at the bundle instead of
  serving it separately:

  ```bash
  SEPIA_UI_DIR=/path/to/node_modules/sepia-ui/dist sepia serve
  ```

- **Separate origin** — host `dist/` anywhere and add the node's URL under
  Settings → Nodes. The API needs `SEPIA_ORIGINS` to allow the UI's origin
  (and `SEPIA_TOKEN` when the node isn't on loopback).

SPA fallback: every non-file path must serve `index.html` (client-side
routing). `/assets/*` is content-hashed and cacheable forever.
