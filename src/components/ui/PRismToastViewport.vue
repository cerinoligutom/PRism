<script setup lang="ts">
import { useToastStore, type ToastVariant } from "@/stores/toast";

const store = useToastStore();

function variantClass(v: ToastVariant): string {
  return `toast-item--${v}`;
}
</script>

<template>
  <Teleport to="body">
    <div class="toast-viewport" role="region" aria-label="Notifications">
      <TransitionGroup name="toast" tag="div" class="toast-stack">
        <button
          v-for="toast in store.toasts"
          :key="toast.id"
          type="button"
          class="toast-item"
          :class="variantClass(toast.variant)"
          :aria-label="`Dismiss notification: ${toast.message}`"
          @click="store.dismiss(toast.id)"
        >
          <span class="toast-item__text">{{ toast.message }}</span>
        </button>
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<style>
/* Viewport is unscoped so callers can target / adjust without piercing
 * `:deep`. The button styles use design tokens so theme + accent swaps
 * propagate without local overrides. */
.toast-viewport {
  position: fixed;
  bottom: 44px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 100;
  pointer-events: none;
  width: max-content;
  max-width: calc(100vw - 64px);
}

.toast-stack {
  display: flex;
  flex-direction: column;
  gap: 8px;
  align-items: center;
}

.toast-item {
  pointer-events: auto;
  display: inline-flex;
  align-items: center;
  padding: 10px 16px;
  border-radius: var(--r-3);
  font-size: var(--fs-12);
  font-weight: 500;
  background: var(--bg-4);
  color: var(--text-strong);
  border: 1px solid var(--border-2);
  box-shadow: 0 8px 24px oklch(0 0 0 / 0.22);
  cursor: pointer;
  max-width: 480px;
  text-align: center;
  font-family: inherit;
  transition: transform 0.12s, box-shadow 0.12s;
}

.toast-item:hover {
  transform: translateY(-1px);
  box-shadow: 0 12px 28px oklch(0 0 0 / 0.28);
}

.toast-item:focus-visible {
  outline: none;
  box-shadow: 0 8px 24px oklch(0 0 0 / 0.22), 0 0 0 2px var(--focus-ring);
}

.toast-item__text {
  white-space: pre-line;
  line-height: 1.4;
}

.toast-item--success {
  background: var(--success-bg);
  color: var(--success);
  border-color: color-mix(in oklch, var(--success) 30%, transparent);
}

.toast-item--info {
  background: var(--info-bg);
  color: var(--info);
  border-color: color-mix(in oklch, var(--info) 30%, transparent);
}

.toast-item--warning {
  background: var(--warning-bg);
  color: var(--warning);
  border-color: color-mix(in oklch, var(--warning) 30%, transparent);
}

.toast-item--danger {
  background: var(--danger-bg);
  color: var(--danger);
  border-color: color-mix(in oklch, var(--danger) 30%, transparent);
}

/* Stack entry / exit animation. New items slide up from the bottom; dismissed
 * items fade out in place so the click-to-dismiss feedback is immediate. */
.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.18s, transform 0.18s;
}

.toast-enter-from {
  opacity: 0;
  transform: translateY(8px);
}

.toast-leave-to {
  opacity: 0;
  transform: scale(0.96);
}

.toast-move {
  transition: transform 0.2s;
}
</style>
