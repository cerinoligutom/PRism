<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";

import PRismButton from "@/components/ui/PRismButton.vue";
import PRismCallout from "@/components/ui/PRismCallout.vue";
import PRismInput from "@/components/ui/PRismInput.vue";
import { useAccountsStore, type Account, type ValidateTokenResult } from "@/stores/accounts";
import { useSyncStore, type AccountSyncState } from "@/stores/sync";

type StepIndex = 1 | 2 | 3;
type TokenFlavour = "fine-grained" | "classic";
type ValidationState =
  | { kind: "idle" }
  | { kind: "validating" }
  | { kind: "valid"; result: ValidateTokenResult }
  | { kind: "error"; message: string };

const router = useRouter();
const accountsStore = useAccountsStore();
const syncStore = useSyncStore();

const currentStep = ref<StepIndex>(1);
const submitting = ref(false);
const newAccount = ref<Account | null>(null);

const form = reactive({
  flavour: "fine-grained" as TokenFlavour,
  label: "",
  host: "github.com",
  token: "",
});

const validation = ref<ValidationState>({ kind: "idle" });

const canConnect = computed(() => {
  return (
    form.label.trim().length > 0 &&
    form.host.trim().length > 0 &&
    form.token.trim().length > 0 &&
    !submitting.value
  );
});

const syncStateForNewAccount = computed<AccountSyncState | null>(() => {
  if (newAccount.value === null) return null;
  return syncStore.accounts.find((a) => a.account_id === newAccount.value!.id) ?? null;
});

const syncDisplay = computed<{ label: string; spinning: boolean }>(() => {
  const state = syncStateForNewAccount.value;
  const host = newAccount.value?.host ?? "github.com";
  // No state yet — the worker hot-add hook fires after addAccount returns,
  // but the sync-status event may not have arrived this tick.
  if (state === null || state.phase === "idle") {
    return { label: "Starting first sync…", spinning: true };
  }
  if (state.phase === "syncing") {
    return { label: `Fetching authored PRs from ${host}`, spinning: true };
  }
  if (state.phase === "synced") {
    return { label: "First sync complete", spinning: false };
  }
  if (state.phase === "unauthorized") {
    return { label: "Token rejected — re-check the PAT", spinning: false };
  }
  if (state.phase === "rate_limited") {
    return { label: "Rate-limited — sync paused", spinning: false };
  }
  return { label: state.message ?? "Sync error — see status bar", spinning: false };
});

function goTo(step: StepIndex): void {
  currentStep.value = step;
  if (step === 2) {
    validation.value = { kind: "idle" };
  }
}

async function handleValidate(): Promise<void> {
  if (form.token.trim().length === 0) {
    validation.value = { kind: "error", message: "Paste a Personal Access Token first." };
    return;
  }
  validation.value = { kind: "validating" };
  try {
    const result = await accountsStore.validateToken(form.host.trim(), form.token.trim());
    validation.value = { kind: "valid", result };
  } catch (err) {
    validation.value = {
      kind: "error",
      message: err instanceof Error ? err.message : "Validation failed.",
    };
  }
}

async function handleConnect(): Promise<void> {
  submitting.value = true;
  try {
    const account = await accountsStore.addAccount({
      label: form.label.trim(),
      host: form.host.trim(),
      token: form.token.trim(),
    });
    newAccount.value = account;
    void syncStore.refreshSnapshot();
    goTo(3);
  } catch (err) {
    validation.value = {
      kind: "error",
      message: err instanceof Error ? err.message : "Could not connect.",
    };
  } finally {
    submitting.value = false;
  }
}

function handleAddAnother(): void {
  form.label = "";
  form.token = "";
  validation.value = { kind: "idle" };
  newAccount.value = null;
  goTo(2);
}

function handleFinish(): void {
  void router.push({ name: "dashboard" });
}

const tokenCreateUrl = computed(() => {
  if (form.flavour === "fine-grained") {
    return "https://github.com/settings/personal-access-tokens/new";
  }
  return "https://github.com/settings/tokens/new?scopes=repo,read:org,read:user&description=PRism";
});

onMounted(() => {
  void syncStore.bind();
});

onUnmounted(() => {
  // Leave the sync store bound for other views — it's a singleton.
});
</script>

<template>
  <section class="onboarding">
    <header class="onboarding__header">
      <div class="onboarding-brand">
        <span class="onboarding-brand__mark" aria-hidden="true">
          <svg viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round">
            <line x1="2" y1="16" x2="9.5" y2="16" opacity="0.55" />
            <path d="M16 4 L28 26 L4 26 Z" />
            <line x1="20.5" y1="17.5" x2="30" y2="11" stroke="oklch(0.72 0.18 25)" />
            <line x1="21" y1="19" x2="30" y2="16" stroke="oklch(0.78 0.15 80)" />
            <line x1="21.5" y1="20.5" x2="30" y2="21" stroke="oklch(0.74 0.16 145)" />
            <line x1="22" y1="22" x2="29" y2="26" stroke="oklch(0.72 0.14 320)" />
          </svg>
        </span>
        <span class="onboarding-brand__name">
          <span>PR</span><span class="onboarding-brand__suffix">ism</span>
        </span>
        <span class="onboarding-brand__version">v0.1</span>
      </div>

      <ol class="onboarding-progress" aria-label="Onboarding progress">
        <li
          v-for="step in [1, 2, 3] as StepIndex[]"
          :key="step"
          class="onboarding-progress__item"
          :class="{
            'onboarding-progress__item--active': currentStep === step,
            'onboarding-progress__item--done': currentStep > step,
          }"
          :aria-current="currentStep === step ? 'step' : undefined"
        >
          <span class="onboarding-progress__num">
            <template v-if="currentStep > step">
              <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 8.5l3 3 7-7" />
              </svg>
            </template>
            <template v-else>{{ step }}</template>
          </span>
          <span class="onboarding-progress__label">
            {{ step === 1 ? "Welcome" : step === 2 ? "Connect" : "First sync" }}
          </span>
        </li>
      </ol>
    </header>

    <div class="onboarding__body">
      <!-- STEP 1: Welcome -->
      <section v-if="currentStep === 1" class="onboarding-step onboarding-step--1">
        <div class="onboarding-step__head">
          <span class="onboarding-step__num onboarding-step__num--active">1</span>
          Welcome
        </div>
        <h2 class="onboarding-step__title">Every PR you touch, in one quiet place.</h2>
        <p class="onboarding-step__lede">
          PRism is a local desktop app that watches every pull request you care about and
          shows you their real state at a glance — conversation depth, reviewer status,
          time-in-status. Nothing leaves your machine.
        </p>

        <div class="onboarding-bullets">
          <div class="onboarding-bullet">
            <span class="onboarding-bullet__icon" style="color: var(--success)">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
                <path d="M8 1.5l5 2.5v3.5c0 3-2.2 5.4-5 6-2.8-.6-5-3-5-6V4l5-2.5z" />
              </svg>
            </span>
            <div>
              <h4 class="onboarding-bullet__title">Local-first, read-only</h4>
              <p class="onboarding-bullet__copy">
                Your PAT lives in the OS keychain. PRism never writes to GitHub — every action
                opens GitHub in your browser.
              </p>
            </div>
          </div>
          <div class="onboarding-bullet">
            <span class="onboarding-bullet__icon" style="color: var(--info)">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M2 8h3l2-5 2 10 2-5h3" />
              </svg>
            </span>
            <div>
              <h4 class="onboarding-bullet__title">Auto-tracking</h4>
              <p class="onboarding-bullet__copy">
                The moment you comment, get a review request, or are @mentioned, PRism picks
                it up.
              </p>
            </div>
          </div>
          <div class="onboarding-bullet">
            <span class="onboarding-bullet__icon" style="color: var(--accent)">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8">
                <circle cx="8" cy="8" r="5.5" />
                <circle cx="8" cy="8" r="2" fill="currentColor" />
              </svg>
            </span>
            <div>
              <h4 class="onboarding-bullet__title">Conversation depth</h4>
              <p class="onboarding-bullet__copy">
                See unresolved threads, oldest open age, and average response time — beyond a
                bare comment count.
              </p>
            </div>
          </div>
        </div>

        <div class="onboarding-step__foot">
          <PRismButton variant="primary" size="lg" @click="goTo(2)">
            Connect GitHub
          </PRismButton>
        </div>
      </section>

      <!-- STEP 2: PAT entry -->
      <section v-else-if="currentStep === 2" class="onboarding-step onboarding-step--2">
        <div class="onboarding-step__head">
          <span class="onboarding-step__num onboarding-step__num--active">2</span>
          Connect an account
        </div>
        <h2 class="onboarding-step__title">Add a Personal Access Token.</h2>
        <p class="onboarding-step__lede">
          PRism authenticates with a <strong>classic</strong> or <strong>fine-grained</strong>
          PAT. Fine-grained is recommended — narrower scope, explicit repository selection.
        </p>

        <div class="onboarding-form">
          <div class="onboarding-tabs" role="tablist">
            <button
              class="onboarding-tabs__btn"
              :class="{ 'onboarding-tabs__btn--active': form.flavour === 'fine-grained' }"
              type="button"
              role="tab"
              :aria-selected="form.flavour === 'fine-grained'"
              @click="form.flavour = 'fine-grained'"
            >
              Fine-grained · recommended
            </button>
            <button
              class="onboarding-tabs__btn"
              :class="{ 'onboarding-tabs__btn--active': form.flavour === 'classic' }"
              type="button"
              role="tab"
              :aria-selected="form.flavour === 'classic'"
              @click="form.flavour = 'classic'"
            >
              Classic
            </button>
          </div>

          <div class="onboarding-field">
            <label for="onb-label" class="onboarding-field__label">Account label</label>
            <PRismInput
              id="onb-label"
              v-model="form.label"
              size="lg"
              placeholder="e.g. Work, Personal"
              :spellcheck="false"
            />
          </div>

          <div class="onboarding-field">
            <label for="onb-host" class="onboarding-field__label">Host</label>
            <PRismInput
              id="onb-host"
              v-model="form.host"
              size="lg"
              mono
              :spellcheck="false"
              autocomplete="off"
            />
            <p class="onboarding-field__hint">
              For GitHub Enterprise use <code>github.your-company.com</code>.
            </p>
          </div>

          <div class="onboarding-field">
            <label for="onb-token" class="onboarding-field__label">Personal Access Token</label>
            <PRismInput
              id="onb-token"
              v-model="form.token"
              size="lg"
              mono
              type="password"
              placeholder="github_pat_… or ghp_…"
              :spellcheck="false"
              autocomplete="off"
            />
            <p class="onboarding-field__hint">
              <a :href="tokenCreateUrl" target="_blank" rel="noreferrer">
                Create a new {{ form.flavour }} PAT
              </a>
              · scopes pre-filled
            </p>
          </div>
        </div>

        <section class="onboarding-scopes" aria-labelledby="onb-scopes-h">
          <header class="onboarding-scopes__head">
            <span id="onb-scopes-h">Required permissions</span>
            <span class="badge badge-success onboarding-scopes__flavour">
              {{ form.flavour === "fine-grained" ? "FINE-GRAINED" : "CLASSIC" }}
            </span>
          </header>
          <div class="onboarding-scope">
            <span class="onboarding-scope__check" aria-hidden="true">
              <svg width="9" height="9" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 8.5l3 3 7-7" />
              </svg>
            </span>
            <div>
              <div class="onboarding-scope__name">Contents</div>
              <div class="onboarding-scope__desc">Read pull request commits and refs.</div>
            </div>
            <span class="onboarding-scope__tag">READ</span>
          </div>
          <div class="onboarding-scope">
            <span class="onboarding-scope__check" aria-hidden="true">
              <svg width="9" height="9" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 8.5l3 3 7-7" />
              </svg>
            </span>
            <div>
              <div class="onboarding-scope__name">Pull requests</div>
              <div class="onboarding-scope__desc">Read PRs, reviews, threads, timelines.</div>
            </div>
            <span class="onboarding-scope__tag">READ</span>
          </div>
          <div class="onboarding-scope">
            <span class="onboarding-scope__check" aria-hidden="true">
              <svg width="9" height="9" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 8.5l3 3 7-7" />
              </svg>
            </span>
            <div>
              <div class="onboarding-scope__name">Metadata</div>
              <div class="onboarding-scope__desc">Read repo info — owner, name, visibility.</div>
            </div>
            <span class="onboarding-scope__tag">READ</span>
          </div>
          <div class="onboarding-scope">
            <span class="onboarding-scope__check" aria-hidden="true">
              <svg width="9" height="9" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 8.5l3 3 7-7" />
              </svg>
            </span>
            <div>
              <div class="onboarding-scope__name">Members (org)</div>
              <div class="onboarding-scope__desc">Resolve org members for team views.</div>
            </div>
            <span class="onboarding-scope__tag">READ</span>
          </div>
        </section>

        <PRismCallout variant="accent">
          <template #icon>
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">
              <path d="M8 1.5l5 2.5v3.5c0 3-2.2 5.4-5 6-2.8-.6-5-3-5-6V4l5-2.5z" />
            </svg>
          </template>
          <strong>PRism never writes to GitHub.</strong>
          Approve / comment / merge actions all open GitHub itself. Your PAT goes straight into
          the OS keychain — never to disk, never to a log.
        </PRismCallout>

        <div v-if="validation.kind === 'error'" class="onboarding-message onboarding-message--error">
          {{ validation.message }}
        </div>

        <div class="onboarding-step__foot">
          <PRismButton @click="goTo(1)">Back</PRismButton>
          <PRismButton variant="primary" size="lg" :disabled="!canConnect" @click="handleConnect">
            <span v-if="submitting">Connecting…</span>
            <span v-else>Connect</span>
          </PRismButton>
          <button
            class="onboarding-validate"
            type="button"
            :disabled="form.token.trim().length === 0 || validation.kind === 'validating'"
            @click="handleValidate"
          >
            <span v-if="validation.kind === 'idle'">Validate first</span>
            <span v-else-if="validation.kind === 'validating'">Validating…</span>
            <span v-else-if="validation.kind === 'valid'" class="onboarding-validate__ok">
              <span class="dot dot-success"></span>
              Token valid · {{ validation.result.login }}
            </span>
            <span v-else>Validate first</span>
          </button>
        </div>
      </section>

      <!-- STEP 3: First sync -->
      <section v-else class="onboarding-step onboarding-step--3">
        <div class="onboarding-step__head">
          <span class="onboarding-step__num onboarding-step__num--active">3</span>
          You're in
        </div>
        <h2 class="onboarding-step__title">Account connected.</h2>
        <p class="onboarding-step__lede">
          <strong>{{ newAccount?.login }}</strong> on
          <code>{{ newAccount?.host }}</code> is saved. Until the full org / repo picker
          lands, every authored PR you can see on github.com appears in the dashboard.
        </p>

        <div class="onboarding-sync">
          <div
            v-if="syncDisplay.spinning"
            class="onboarding-sync__ring"
            aria-hidden="true"
          ></div>
          <div v-else class="onboarding-sync__dot" aria-hidden="true">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="8" cy="8" r="6" />
              <path d="M5.5 8l2 2 3-3.5" />
            </svg>
          </div>
          <div class="onboarding-sync__text">{{ syncDisplay.label }}</div>
        </div>

        <PRismCallout variant="info">
          <template #icon>
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8">
              <circle cx="8" cy="8" r="6" />
              <path d="M8 7v4M8 5v.5" />
            </svg>
          </template>
          The first sync may be slower than later ones; subsequent fetches use conditional
          requests so most responses are 304 Not Modified.
        </PRismCallout>

        <div class="onboarding-step__foot">
          <PRismButton @click="handleAddAnother">Add another account</PRismButton>
          <PRismButton variant="primary" size="lg" @click="handleFinish">
            Open PRism
          </PRismButton>
        </div>
      </section>
    </div>
  </section>
</template>

<style scoped>
.onboarding {
  height: 100%;
  background: var(--bg-1);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.onboarding__header {
  display: flex;
  align-items: center;
  gap: var(--s-6);
  padding: var(--s-5) var(--s-8);
  border-bottom: 1px solid var(--border-1);
  background: var(--bg-1);
  flex: 0 0 auto;
}

.onboarding-brand {
  display: flex;
  align-items: center;
  gap: var(--s-3);
}

.onboarding-brand__mark {
  width: 24px;
  height: 24px;
  color: var(--text-strong);
}

.onboarding-brand__name {
  font-size: var(--fs-14);
  font-weight: 600;
  letter-spacing: -0.4px;
  color: var(--text-strong);
}

.onboarding-brand__suffix {
  font-weight: 400;
  color: var(--text-mute);
}

.onboarding-brand__version {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  letter-spacing: 1px;
}

.onboarding-progress {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  align-items: center;
  gap: var(--s-5);
}

.onboarding-progress__item {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  text-transform: uppercase;
  letter-spacing: 1px;
  color: var(--text-faint);
  position: relative;
}

.onboarding-progress__item + .onboarding-progress__item::before {
  content: "";
  width: 28px;
  height: 1px;
  background: var(--border-2);
  margin-right: var(--s-3);
}

.onboarding-progress__num {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  background: var(--bg-3);
  color: var(--text-mute);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: var(--fs-10);
  border: 1px solid var(--border-2);
}

.onboarding-progress__item--active .onboarding-progress__num {
  background: var(--accent);
  color: var(--accent-fg);
  border-color: transparent;
  font-weight: 600;
}

.onboarding-progress__item--active .onboarding-progress__label {
  color: var(--text);
}

.onboarding-progress__item--done .onboarding-progress__num {
  background: var(--success);
  color: #001a08;
  border-color: transparent;
}

.onboarding__body {
  flex: 1 1 auto;
  min-height: 0;
  overflow: auto;
  display: flex;
  justify-content: center;
}

.onboarding-step {
  width: 100%;
  max-width: 640px;
  padding: var(--s-7) var(--s-8) var(--s-7);
  display: flex;
  flex-direction: column;
  gap: var(--s-5);
  position: relative;
}

.onboarding-step--1::before {
  content: "";
  position: absolute;
  inset: 0;
  background:
    radial-gradient(50% 40% at 30% 20%, oklch(0.32 0.08 var(--accent-h) / 0.4), transparent 70%),
    radial-gradient(40% 35% at 90% 95%, oklch(0.28 0.12 25 / 0.3), transparent 70%);
  pointer-events: none;
}

.onboarding-step--1 > * {
  position: relative;
  z-index: 1;
}

.onboarding-step__head {
  display: flex;
  align-items: center;
  gap: 10px;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  letter-spacing: 1.2px;
  text-transform: uppercase;
}

.onboarding-step__num {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  background: var(--bg-3);
  color: var(--text-mute);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: var(--fs-10);
  border: 1px solid var(--border-2);
}

.onboarding-step__num--active {
  background: var(--accent);
  color: var(--accent-fg);
  border-color: transparent;
  font-weight: 600;
}

.onboarding-step__title {
  margin: 0;
  font-size: 26px;
  font-weight: 600;
  letter-spacing: -0.6px;
  color: var(--text-strong);
  line-height: var(--lh-tight);
}

.onboarding-step__lede {
  color: var(--text-mute);
  font-size: var(--fs-13);
  line-height: var(--lh-body);
  margin: calc(-1 * var(--s-3)) 0 0;
  max-width: 540px;
}

.onboarding-step__lede :deep(strong) {
  color: var(--text);
  font-weight: 600;
}

.onboarding-step__lede :deep(code) {
  color: var(--text);
  font-family: var(--font-mono);
  font-size: var(--fs-12);
}

.onboarding-bullets {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
  margin-top: var(--s-2);
}

.onboarding-bullet {
  display: grid;
  grid-template-columns: 32px 1fr;
  gap: var(--s-3);
  align-items: flex-start;
}

.onboarding-bullet__icon {
  width: 28px;
  height: 28px;
  border-radius: var(--r-2);
  background: var(--bg-3);
  border: 1px solid var(--border-1);
  display: flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 28px;
}

.onboarding-bullet__title {
  margin: 0 0 4px;
  font-size: var(--fs-13);
  font-weight: 600;
  color: var(--text-strong);
}

.onboarding-bullet__copy {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
}

.onboarding-step__foot {
  margin-top: var(--s-3);
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}

.onboarding-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin-top: calc(-1 * var(--s-1));
}

.onboarding-tabs {
  display: inline-flex;
  background: var(--bg-2);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  overflow: hidden;
  align-self: flex-start;
}

.onboarding-tabs__btn {
  background: transparent;
  border: 0;
  border-right: 1px solid var(--border-1);
  color: var(--text-mute);
  padding: 0 12px;
  height: 28px;
  font-size: var(--fs-11);
  font-weight: 500;
  cursor: pointer;
}

.onboarding-tabs__btn:last-child {
  border-right: 0;
}

.onboarding-tabs__btn--active {
  color: var(--text-strong);
  background: var(--bg-4);
}

.onboarding-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.onboarding-field__label {
  font-family: var(--font-mono);
  font-size: var(--fs-12);
  text-transform: uppercase;
  letter-spacing: 1px;
  color: var(--text-mute);
}

.onboarding-field__hint {
  margin: 2px 0 0;
  color: var(--text-mute);
  font-size: var(--fs-12);
}

.onboarding-field__hint a {
  color: var(--accent);
  text-decoration: none;
}

.onboarding-field__hint code {
  color: var(--text);
  font-family: var(--font-mono);
}

.onboarding-scopes {
  background: var(--bg-2);
  border: 1px solid var(--border-1);
  border-radius: var(--r-3);
  overflow: hidden;
}

.onboarding-scopes__head {
  padding: 10px 14px;
  background: var(--bg-3);
  border-bottom: 1px solid var(--border-1);
  display: flex;
  align-items: center;
  gap: var(--s-2);
  font-size: var(--fs-11);
  color: var(--text);
}

.onboarding-scopes__flavour {
  margin-left: auto;
  height: 16px;
  font-size: var(--fs-9);
}

.onboarding-scope {
  display: grid;
  grid-template-columns: 16px 1fr auto;
  gap: var(--s-3);
  align-items: center;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-1);
}

.onboarding-scope:last-child {
  border-bottom: 0;
}

.onboarding-scope__check {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: var(--success-bg);
  color: var(--success);
  display: flex;
  align-items: center;
  justify-content: center;
}

.onboarding-scope__name {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text);
}

.onboarding-scope__desc {
  color: var(--text-mute);
  font-size: var(--fs-11);
}

.onboarding-scope__tag {
  font-family: var(--font-mono);
  font-size: var(--fs-9);
  color: var(--text-faint);
  text-transform: uppercase;
  letter-spacing: 0.6px;
  padding: 1px 6px;
  background: var(--bg-4);
  border-radius: var(--r-1);
}

.onboarding-validate {
  margin-left: auto;
  background: transparent;
  border: 0;
  font-size: var(--fs-11);
  color: var(--text-faint);
  display: inline-flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  padding: 0;
}

.onboarding-validate:disabled {
  cursor: not-allowed;
  opacity: 0.6;
}

.onboarding-validate:hover:not(:disabled) {
  color: var(--text);
}

.onboarding-validate__ok {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--success);
}

.onboarding-message {
  font-size: var(--fs-12);
  padding: 10px 14px;
  border-radius: var(--r-2);
}

.onboarding-message--error {
  background: var(--danger-bg);
  color: var(--danger);
  border: 1px solid oklch(0.4 0.12 25 / 0.4);
}

.onboarding-sync {
  background: var(--bg-2);
  border: 1px solid var(--border-1);
  border-radius: var(--r-3);
  padding: 12px 14px;
  display: flex;
  align-items: center;
  gap: 10px;
}

.onboarding-sync__ring {
  width: 22px;
  height: 22px;
  border: 2px solid var(--border-2);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: onboarding-sync-spin 1.4s linear infinite;
}

.onboarding-sync__dot {
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-mute);
}

@keyframes onboarding-sync-spin {
  to {
    transform: rotate(360deg);
  }
}

.onboarding-sync__text {
  flex: 1;
  font-size: var(--fs-12);
  color: var(--text);
}
</style>
