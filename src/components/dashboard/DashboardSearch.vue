<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";
import { useEventListener } from "@vueuse/core";

interface Props {
  modelValue: string;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

const inputRef = ref<HTMLInputElement | null>(null);

/** Local mirror of the input value so the user sees keystrokes immediately;
 * the debounced `update:modelValue` propagates the value upstream at most
 * once per `DEBOUNCE_MS`. Parent-driven changes (e.g. view-change reset)
 * write back through the watcher below. */
const local = ref<string>(props.modelValue);

const DEBOUNCE_MS = 150;
let pendingTimer: ReturnType<typeof setTimeout> | null = null;

function flush(): void {
  if (pendingTimer !== null) {
    clearTimeout(pendingTimer);
    pendingTimer = null;
  }
}

function schedule(value: string): void {
  flush();
  pendingTimer = setTimeout(() => {
    pendingTimer = null;
    emit("update:modelValue", value);
  }, DEBOUNCE_MS);
}

function onInput(event: Event): void {
  const target = event.target as HTMLInputElement;
  local.value = target.value;
  schedule(target.value);
}

/** Keep the local value in sync when the parent rewrites `modelValue`
 * (e.g. view change clearing the query). Skip when the parent's update is
 * already what we hold, otherwise we'd cancel an in-flight debounce. */
watch(
  () => props.modelValue,
  (next) => {
    if (next === local.value) return;
    local.value = next;
    flush();
  },
);

/**
 * `cmd+K` on macOS, `ctrl+K` everywhere else. Bound to `window` via
 * `useEventListener` so the focus call lands no matter where focus currently
 * sits inside the dashboard. `useEventListener` cleans itself up on unmount;
 * the explicit `onBeforeUnmount` below only handles the pending debounce.
 */
useEventListener(window, "keydown", (event: KeyboardEvent) => {
  if (event.key !== "k" && event.key !== "K") return;
  const isMacCombo = event.metaKey && !event.ctrlKey;
  const isNonMacCombo = event.ctrlKey && !event.metaKey;
  if (!isMacCombo && !isNonMacCombo) return;
  event.preventDefault();
  inputRef.value?.focus();
  inputRef.value?.select();
});

function focus(): void {
  inputRef.value?.focus();
}

/**
 * Surface the platform-appropriate modifier glyph. The artboard shows the
 * Apple Command symbol; on Windows / Linux we render "Ctrl" so the hint
 * matches the binding the `keydown` listener actually responds to.
 */
const cmdGlyph = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl";

onBeforeUnmount(() => {
  flush();
});

defineExpose({ focus });
</script>

<template>
  <div class="search">
    <span class="search__icon" aria-hidden="true">
      <svg
        width="13"
        height="13"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
      >
        <circle cx="7" cy="7" r="4.5" />
        <path d="M11 11l3 3" stroke-linecap="round" />
      </svg>
    </span>
    <input
      ref="inputRef"
      type="search"
      class="search__input"
      placeholder="Search PRs, repos, authors..."
      aria-label="Search pull requests"
      :value="local"
      @input="onInput"
    />
    <span class="search__kbds" aria-hidden="true">
      <kbd>{{ cmdGlyph }}</kbd>
      <kbd>K</kbd>
    </span>
  </div>
</template>

<style scoped>
.search {
  position: relative;
  flex: 1 1 auto;
  min-width: 0;
  max-width: 360px;
}

.search__input {
  width: 100%;
  height: 30px;
  padding-left: 32px;
  padding-right: 70px;
  background: var(--bg-2);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  color: var(--text);
  font-size: var(--fs-12);
  outline: none;
  transition: border-color 0.12s, box-shadow 0.12s;
}

.search__input::placeholder {
  color: var(--text-faint);
}

.search__input:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--focus-ring);
}

/* Strip Safari/Chrome's default search-input chrome (the magnifying glass
 * and "x" button) so the custom icon + kbd hints aren't doubled up. */
.search__input::-webkit-search-decoration,
.search__input::-webkit-search-cancel-button,
.search__input::-webkit-search-results-button,
.search__input::-webkit-search-results-decoration {
  appearance: none;
}

.search__icon {
  position: absolute;
  left: 10px;
  top: 50%;
  transform: translateY(-50%);
  color: var(--text-faint);
  display: inline-flex;
  pointer-events: none;
}

.search__kbds {
  position: absolute;
  right: 8px;
  top: 50%;
  transform: translateY(-50%);
  display: inline-flex;
  gap: 2px;
  pointer-events: none;
}
</style>
