import { defineConfig } from "vite";
import { resolve } from "node:path";

// SITE_BASE is "/<repo>/" on GitHub Pages, "/" elsewhere.
export default defineConfig({
  base: process.env.SITE_BASE || "/",
  build: {
    rollupOptions: { input: { main: resolve(import.meta.dirname, "index.html"), docs: resolve(import.meta.dirname, "docs/index.html") } },
  },
});
