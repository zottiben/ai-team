import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The bundle is embedded in the binary and served from memory, so the filenames never
// need a content hash - and a stable name keeps the committed dist/ diff readable,
// which matters because it is reviewed like any other file.
export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsDir: ".",
    rollupOptions: {
      output: {
        entryFileNames: "app.js",
        chunkFileNames: "[name].js",
        assetFileNames: "[name][extname]",
      },
    },
  },
  server: {
    // `npm run dev` proxies to a server started with `ait ui --port 7788 --no-open`, so
    // the frontend can hot-reload against real data. Nobody needs this to *use* the
    // tool - the shipped app is served by the binary.
    proxy: {
      "/api": {
        target: "http://127.0.0.1:7788",
        changeOrigin: true,
      },
    },
  },
});
