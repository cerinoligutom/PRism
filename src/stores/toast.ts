import { defineStore } from "pinia";
import { ref } from "vue";

/**
 * Reusable transient toast system. Toasts render in a bottom-right viewport
 * mounted once in `App.vue` (see `PRismToastViewport.vue`); any component
 * can call `useToastStore().show(...)` to enqueue one. Toasts auto-dismiss
 * after their configured `duration` and are also click-to-dismiss.
 *
 * Variants map to the four design-token colour roles (`success`, `info`,
 * `warning`, `danger`) so callers don't reinvent styling per surface. The
 * default is `info` so a bare `show("…")` works without ceremony.
 */
export type ToastVariant = "success" | "info" | "warning" | "danger";

export interface Toast {
  readonly id: string;
  readonly message: string;
  readonly variant: ToastVariant;
  /** Milliseconds before auto-dismiss. `0` = sticky (manual dismiss only). */
  readonly duration: number;
}

export interface ToastOptions {
  readonly variant?: ToastVariant;
  readonly duration?: number;
}

const DEFAULT_DURATION_MS = 3000;

/**
 * Soft cap on the visible stack. A 5th toast bumps the oldest out so the
 * stack can't blanket the viewport during a fast burst (e.g. an org-level
 * "Track all" toggle that fans out per-repo writes). Tune if a real surface
 * needs more.
 */
const MAX_VISIBLE = 4;

export const useToastStore = defineStore("toast", () => {
  const toasts = ref<readonly Toast[]>([]);

  // Per-toast auto-dismiss timers. Kept outside the reactive ref because the
  // timer ids are plumbing - mutating them shouldn't trigger renders.
  const timers = new Map<string, number>();

  function makeId(): string {
    return `t-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  }

  function clearTimer(id: string): void {
    const handle = timers.get(id);
    if (handle !== undefined) {
      window.clearTimeout(handle);
      timers.delete(id);
    }
  }

  /**
   * Enqueue a toast. Returns the toast id so callers can dismiss it
   * programmatically (e.g. a long-running operation that wants to clear its
   * own progress toast on completion).
   */
  function show(message: string, options: ToastOptions = {}): string {
    const id = makeId();
    const toast: Toast = {
      id,
      message,
      variant: options.variant ?? "info",
      duration: options.duration ?? DEFAULT_DURATION_MS,
    };
    let next = toasts.value.slice();
    while (next.length >= MAX_VISIBLE) {
      const dropped = next.shift();
      if (dropped !== undefined) clearTimer(dropped.id);
    }
    next.push(toast);
    toasts.value = next;
    if (toast.duration > 0) {
      const handle = window.setTimeout(() => dismiss(id), toast.duration);
      timers.set(id, handle);
    }
    return id;
  }

  function dismiss(id: string): void {
    clearTimer(id);
    toasts.value = toasts.value.filter((t) => t.id !== id);
  }

  function clear(): void {
    for (const handle of timers.values()) window.clearTimeout(handle);
    timers.clear();
    toasts.value = [];
  }

  return { toasts, show, dismiss, clear };
});
