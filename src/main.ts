import { createApp } from "vue";
import { createPinia } from "pinia";

// Self-hosted fonts (Latin subset only — PRism is English-only). Each import
// brings in one @font-face rule + the corresponding woff2 from node_modules
// at build time, so the app runs fully offline and doesn't reach out to
// fonts.googleapis.com / fonts.gstatic.com at runtime.
import "@fontsource/geist/latin-300.css";
import "@fontsource/geist/latin-400.css";
import "@fontsource/geist/latin-500.css";
import "@fontsource/geist/latin-600.css";
import "@fontsource/geist/latin-700.css";
import "@fontsource/jetbrains-mono/latin-400.css";
import "@fontsource/jetbrains-mono/latin-500.css";
import "@fontsource/jetbrains-mono/latin-600.css";

import App from "./App.vue";
import { router } from "./router";
import { useAppearanceStore } from "./stores/appearance";

import "./assets/styles/main.css";

const app = createApp(App);
const pinia = createPinia();

app.use(pinia);
app.use(router);

// Apply persisted theme/accent/density to <html> before mount so the first
// paint matches the user's last session.
useAppearanceStore().hydrate();

app.mount("#app");
