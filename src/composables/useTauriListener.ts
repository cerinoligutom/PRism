import { tryOnScopeDispose } from "@vueuse/core";
import type { UnlistenFn } from "@tauri-apps/api/event";

export type TauriSubscribe = () => Promise<UnlistenFn[]>;

export interface TauriListener {
  /** Register listeners. Re-entry while already bound is a no-op. */
  bind(subscribe: TauriSubscribe): Promise<void>;
  /** Tear down every registered listener. Safe to call multiple times. */
  unbind(): void;
  /** Whether listeners are currently registered. */
  readonly isBound: () => boolean;
}

/**
 * Manages the lifecycle of one or more Tauri `listen()` subscriptions:
 * idempotent bind, cleanup-all unbind, and an automatic teardown when the
 * caller's effect scope disposes. Lets Pinia stores (and any future
 * component-scoped consumer) drop the bespoke `UnlistenFn[]` book-keeping.
 *
 * Pinia stores still expose their own `unbind()` so consumers can release
 * listeners on view-level lifecycle events (e.g. `onBeforeUnmount` in
 * `StatusBar`). The scope-dispose hook is a backstop.
 */
export function useTauriListener(): TauriListener {
  let unlisteners: UnlistenFn[] = [];

  async function bind(subscribe: TauriSubscribe): Promise<void> {
    if (unlisteners.length > 0) return;
    unlisteners = await subscribe();
  }

  function unbind(): void {
    for (const off of unlisteners) off();
    unlisteners = [];
  }

  tryOnScopeDispose(unbind);

  return {
    bind,
    unbind,
    isBound: () => unlisteners.length > 0,
  };
}
