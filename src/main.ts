import { createApp } from "vue";
import { createPinia } from "pinia";

import App from "./App.vue";
import { router } from "./router";
import { useThemeStore } from "./stores/theme";

import "./assets/styles/main.css";

const app = createApp(App);
const pinia = createPinia();

app.use(pinia);
app.use(router);

// Apply persisted theme/accent/density to <html> before mount so the first
// paint matches the user's last session.
useThemeStore().hydrate();

app.mount("#app");
