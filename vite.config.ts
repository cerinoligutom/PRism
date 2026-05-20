import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// Tauri exposes the dev host on a known env when running `tauri dev`.
// We bind explicitly so the webview can reach Vite over the network.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 5174,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // Tauri's webview baseline: macOS Safari 15+ (macOS 11+) and WebView2
    // (Chromium evergreen) on Windows. Older safari13 / chrome105 targets
    // trip esbuild 0.28 + Vite 8's stricter downlevel checks (e.g. array
    // destructuring assignment in vue-router).
    target:
      process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome120" : "safari15",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
