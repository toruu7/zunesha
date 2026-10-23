import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Static export into `web/out` — the Axum daemon serves it (and falls back
  // to `index.html` for unknown paths), so no Node server is needed at runtime.
  //
  // Note: the create-next-app scaffold enabled `cacheComponents` +
  // `partialPrefetching` (PPR), but PPR is incompatible with export mode, so
  // both are off here. The dashboard is a fully client-side SPA (WebSocket +
  // Zustand), so there's nothing to prerender beyond the shell anyway.
  output: "export",
  turbopack: {
    root: __dirname,
    rules: {
      "*.css": {
        loaders: ["@tailwindcss/turbopack"],
        as: "*.css",
      },
    },
  },
};

export default nextConfig;
