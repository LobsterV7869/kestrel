import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// The frontend is deliberately tiny: no router, no UI kit, no icon package.
// Everything here exists to reach first paint as early as possible.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
    minify: "esbuild",
    cssMinify: true,
    assetsInlineLimit: 4096,
    sourcemap: false,
    reportCompressedSize: false,
  },
});
