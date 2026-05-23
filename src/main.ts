import { createApp } from "vue";
import { createPinia } from "pinia";

// Self-hosted fonts (Latin subset only — PRism is English-only). Each import
// brings in one @font-face rule + the corresponding woff2 from node_modules
// at build time, so the app runs fully offline and doesn't reach out to
// fonts.googleapis.com / fonts.gstatic.com at runtime.
import "@fontsource/mona-sans/latin-300.css";
import "@fontsource/mona-sans/latin-400.css";
import "@fontsource/mona-sans/latin-500.css";
import "@fontsource/mona-sans/latin-600.css";
import "@fontsource/mona-sans/latin-700.css";
// Monospace uses the OS-provided system stack (SF Mono / Consolas / Menlo) -
// matches what users see on github.com, no web font payload.

import App from "./App.vue";
import { router } from "./router";
import { useAppearanceStore } from "./stores/appearance";

import "./assets/styles/main.css";
import "./assets/styles/markdown.css";

const app = createApp(App);
const pinia = createPinia();

app.use(pinia);
app.use(router);

// Apply persisted theme/accent/density to <html> before mount so the first
// paint matches the user's last session.
useAppearanceStore().hydrate();

app.mount("#app");
