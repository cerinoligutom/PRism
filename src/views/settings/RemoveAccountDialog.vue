<script setup lang="ts">
import { computed } from "vue";
import {
  DialogContent,
  DialogDescription,
  DialogOverlay,
  DialogPortal,
  DialogRoot,
  DialogTitle,
} from "reka-ui";

import PRismButton from "@/components/ui/PRismButton.vue";
import { type Account } from "@/stores/accounts";

interface Props {
  /** When non-null, the dialog opens targeting this account. */
  account: Account | null;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  close: [];
  /** Fired when the user confirms removal. Host owns the store call so it
   * can keep its own per-account markers (e.g. expiredAccountIds) in sync. */
  confirm: [];
}>();

// `DialogRoot.open` is two-way bound. The host clears `account` to close;
// Reka's `update:open(false)` on Esc / overlay click / close button relays
// through `close` so the host can null its target ref.
const open = computed<boolean>({
  get: () => props.account !== null,
  set: (next) => {
    if (!next) emit("close");
  },
});

function handleCancel(): void {
  emit("close");
}

function handleConfirm(): void {
  emit("confirm");
}
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="remove-account-modal__overlay" />
      <DialogContent class="remove-account-modal">
        <header class="remove-account-modal__header">
          <DialogTitle class="remove-account-modal__title">
            Remove account
          </DialogTitle>
          <DialogDescription class="remove-account-modal__desc">
            <span v-if="account">
              You're about to remove
              <code>{{ account.label || account.login }}</code> on
              <code>{{ account.host }}</code>.
            </span>
          </DialogDescription>
        </header>

        <div class="remove-account-modal__body">
          <p class="remove-account-modal__warning">
            This will delete the PAT from the OS keychain and clear the
            unread / archive state stored locally for this account. Re-adding
            the same identity later starts from a clean slate.
          </p>

          <footer class="remove-account-modal__foot">
            <PRismButton type="button" @click="handleCancel">
              Cancel
            </PRismButton>
            <PRismButton type="button" variant="danger" @click="handleConfirm">
              Remove
            </PRismButton>
          </footer>
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.remove-account-modal__overlay {
  position: fixed;
  inset: 0;
  background: rgb(0 0 0 / 0.5);
  /* Matches ReauthDialog so destructive surfaces sit at the same depth as
     re-auth above the drawer (60-70) and tooltips (50). */
  z-index: 80;
  animation: remove-account-modal-fade-in 0.14s ease-out;
}

.remove-account-modal__overlay[data-state="closed"] {
  animation: remove-account-modal-fade-out 0.14s ease-in;
}

.remove-account-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: min(440px, calc(100vw - 32px));
  background: var(--bg-1);
  border: 1px solid var(--border-1);
  border-radius: var(--r-3);
  box-shadow: var(--shadow-3);
  z-index: 90;
  display: flex;
  flex-direction: column;
  animation: remove-account-modal-pop-in 0.16s ease-out;
}

.remove-account-modal[data-state="closed"] {
  animation: remove-account-modal-pop-out 0.12s ease-in;
}

.remove-account-modal__header {
  padding: var(--s-5) var(--s-5) var(--s-3);
  border-bottom: 1px solid var(--border-1);
}

.remove-account-modal__title {
  margin: 0;
  font-size: var(--fs-14);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.2px;
}

.remove-account-modal__desc {
  margin: 6px 0 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
}

.remove-account-modal__desc code {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text);
  background: var(--bg-3);
  padding: 1px 5px;
  border-radius: var(--r-1);
}

.remove-account-modal__body {
  padding: var(--s-4) var(--s-5) var(--s-5);
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}

.remove-account-modal__warning {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text);
  line-height: var(--lh-body);
}

.remove-account-modal__foot {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--s-2);
}

@keyframes remove-account-modal-fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}

@keyframes remove-account-modal-fade-out {
  from { opacity: 1; }
  to { opacity: 0; }
}

@keyframes remove-account-modal-pop-in {
  from {
    opacity: 0;
    transform: translate(-50%, calc(-50% + 6px));
  }
  to {
    opacity: 1;
    transform: translate(-50%, -50%);
  }
}

@keyframes remove-account-modal-pop-out {
  from {
    opacity: 1;
    transform: translate(-50%, -50%);
  }
  to {
    opacity: 0;
    transform: translate(-50%, calc(-50% + 6px));
  }
}
</style>
