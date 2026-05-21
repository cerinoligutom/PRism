<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  DialogContent,
  DialogDescription,
  DialogOverlay,
  DialogPortal,
  DialogRoot,
  DialogTitle,
} from "reka-ui";

import PRismButton from "@/components/ui/PRismButton.vue";
import PRismInput from "@/components/ui/PRismInput.vue";
import { useAccountsStore, type Account } from "@/stores/accounts";

interface Props {
  /** When non-null, the dialog opens targeting this account. */
  account: Account | null;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  close: [];
  /** Fired after a successful swap so the host can clear any reauth banner. */
  success: [accountId: number];
}>();

// `DialogRoot.open` is two-way bound. When the host clears `account` we report
// closed; when the user dismisses (Esc, overlay click, close button) Reka emits
// `update:open(false)` and we relay through `close`.
const open = computed<boolean>({
  get: () => props.account !== null,
  set: (next) => {
    if (!next) emit("close");
  },
});

const accountsStore = useAccountsStore();

type Submission =
  | { kind: "idle" }
  | { kind: "submitting" }
  | { kind: "error"; message: string };

const token = ref("");
const submission = ref<Submission>({ kind: "idle" });

const canSubmit = computed(() => {
  return token.value.trim().length > 0 && submission.value.kind !== "submitting";
});

// Reset transient state whenever the dialog opens against a different (or
// freshly-opened) account so a previous error message doesn't bleed into the
// next session. Reka's DialogContent traps focus on mount, so the PAT input
// receives focus automatically as the first interactive child.
watch(
  () => props.account?.id ?? null,
  (id) => {
    if (id !== null) {
      token.value = "";
      submission.value = { kind: "idle" };
    }
  },
);

async function handleSubmit(): Promise<void> {
  const account = props.account;
  if (account === null) return;
  const value = token.value.trim();
  if (value.length === 0) return;

  submission.value = { kind: "submitting" };
  try {
    await accountsStore.updateToken(account.id, value);
    emit("success", account.id);
    emit("close");
  } catch (err) {
    submission.value = {
      kind: "error",
      message: err instanceof Error ? err.message : "Could not re-authenticate.",
    };
  }
}

function handleCancel(): void {
  emit("close");
}
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="reauth-modal__overlay" />
      <DialogContent class="reauth-modal">
        <header class="reauth-modal__header">
          <DialogTitle class="reauth-modal__title">
            Re-authenticate
            <span v-if="account" class="reauth-modal__title-account">{{ account.label || account.login }}</span>
          </DialogTitle>
          <DialogDescription class="reauth-modal__desc">
            Paste a fresh PAT for
            <code v-if="account">{{ account.login }}</code> on
            <code v-if="account">{{ account.host }}</code>.
            The host and identity are immutable on this row: to switch
            account, remove this one and add it again.
          </DialogDescription>
        </header>

        <form class="reauth-modal__body" @submit.prevent="handleSubmit">
          <div class="reauth-modal__field">
            <label for="reauth-token-input" class="reauth-modal__label">
              Personal Access Token
            </label>
            <PRismInput
              id="reauth-token-input"
              v-model="token"
              size="lg"
              mono
              type="password"
              placeholder="github_pat_… or ghp_…"
              :spellcheck="false"
              autocomplete="off"
              :disabled="submission.kind === 'submitting'"
            />
          </div>

          <div
            v-if="submission.kind === 'error'"
            class="reauth-modal__error"
            role="alert"
          >
            {{ submission.message }}
          </div>

          <footer class="reauth-modal__foot">
            <PRismButton
              type="button"
              :disabled="submission.kind === 'submitting'"
              @click="handleCancel"
            >
              Cancel
            </PRismButton>
            <PRismButton
              type="submit"
              variant="primary"
              :disabled="!canSubmit"
            >
              <span v-if="submission.kind === 'submitting'">Validating…</span>
              <span v-else>Validate &amp; save</span>
            </PRismButton>
          </footer>
        </form>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.reauth-modal__overlay {
  position: fixed;
  inset: 0;
  background: rgb(0 0 0 / 0.5);
  /* Layered above the PR drawer (z-index 60-70) and tooltips (50) so the
     re-auth surface always wins focus when invoked from any settings view. */
  z-index: 80;
  animation: reauth-modal-fade-in 0.14s ease-out;
}

.reauth-modal__overlay[data-state="closed"] {
  animation: reauth-modal-fade-out 0.14s ease-in;
}

.reauth-modal {
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
  animation: reauth-modal-pop-in 0.16s ease-out;
}

.reauth-modal[data-state="closed"] {
  animation: reauth-modal-pop-out 0.12s ease-in;
}

.reauth-modal__header {
  padding: var(--s-5) var(--s-5) var(--s-3);
  border-bottom: 1px solid var(--border-1);
}

.reauth-modal__title {
  margin: 0;
  font-size: var(--fs-14);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.2px;
  display: flex;
  align-items: baseline;
  gap: var(--s-2);
  flex-wrap: wrap;
}

.reauth-modal__title-account {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-mute);
  letter-spacing: 0;
  font-weight: 400;
}

.reauth-modal__desc {
  margin: 6px 0 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
}

.reauth-modal__desc code {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text);
  background: var(--bg-3);
  padding: 1px 5px;
  border-radius: var(--r-1);
}

.reauth-modal__body {
  padding: var(--s-4) var(--s-5) var(--s-5);
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}

.reauth-modal__field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.reauth-modal__label {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  text-transform: uppercase;
  letter-spacing: 1px;
  color: var(--text-mute);
}

.reauth-modal__error {
  font-size: var(--fs-12);
  padding: 10px 12px;
  border-radius: var(--r-2);
  background: var(--danger-bg);
  color: var(--danger);
  border: 1px solid oklch(0.4 0.12 25 / 0.4);
  line-height: var(--lh-body);
}

.reauth-modal__foot {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--s-2);
}

@keyframes reauth-modal-fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}

@keyframes reauth-modal-fade-out {
  from { opacity: 1; }
  to { opacity: 0; }
}

@keyframes reauth-modal-pop-in {
  from {
    opacity: 0;
    transform: translate(-50%, calc(-50% + 6px));
  }
  to {
    opacity: 1;
    transform: translate(-50%, -50%);
  }
}

@keyframes reauth-modal-pop-out {
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
