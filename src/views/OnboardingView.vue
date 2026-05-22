<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import { useRouter } from "vue-router";

import ScopeStateIcon from "@/components/onboarding/ScopeStateIcon.vue";
import ScopeStateTag from "@/components/onboarding/ScopeStateTag.vue";
import PRismButton from "@/components/ui/PRismButton.vue";
import PRismCallout from "@/components/ui/PRismCallout.vue";
import PRismInput from "@/components/ui/PRismInput.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
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
const connectError = ref<string | null>(null);

// Monotonic id so a stale in-flight validation can't overwrite the
// state of a newer one — incremented on every kick-off and on token
// changes, only the latest id is allowed to commit a result.
let validationToken = 0;

/// Read the leading characters of a pasted PAT to identify which kind it is.
/// `github_pat_` is fine-grained, `ghp_` is the classic prefix. Returns null
/// for unknown / legacy (pre-prefix) tokens, the empty string, or non-PAT
/// token types (`gho_` / `ghu_` / `ghs_`); detection failure leaves the tab
/// as the source of truth.
function detectPatFlavour(token: string): TokenFlavour | null {
  const trimmed = token.trim();
  if (trimmed.startsWith("github_pat_")) return "fine-grained";
  if (trimmed.startsWith("ghp_")) return "classic";
  return null;
}

const detectedFlavour = computed<TokenFlavour | null>(() =>
  detectPatFlavour(form.token),
);

/// Effective flavour drives gating and the "Create a new PAT" link. The tab
/// only controls which help text is rendered; the actual pasted PAT is what
/// gets connected, so the gate has to follow the detected type when it's
/// known. Falls back to the tab when detection fails (legacy 40-hex
/// classics, empty field).
const effectiveFlavour = computed<TokenFlavour>(
  () => detectedFlavour.value ?? form.flavour,
);

const permissionsSatisfied = computed(() => {
  if (validation.value.kind !== "valid") return false;
  if (effectiveFlavour.value === "fine-grained") {
    // GitHub doesn't expose granted permissions for fine-grained PATs
    // through any documented endpoint, so we don't gate Connect on
    // per-permission verification. Token validity is the only gate.
    return true;
  }
  return classicScopes
    .filter((s) => s.required === true)
    .every((s) => rowStateForScope(s.name) === "granted");
});

const canConnect = computed(() => {
  return (
    form.label.trim().length > 0 &&
    form.host.trim().length > 0 &&
    form.token.trim().length > 0 &&
    validation.value.kind === "valid" &&
    permissionsSatisfied.value &&
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
  const token = form.token.trim();
  const host = form.host.trim();
  if (token.length === 0 || host.length === 0) {
    validation.value = { kind: "idle" };
    return;
  }
  const ticket = ++validationToken;
  validation.value = { kind: "validating" };
  try {
    const result = await accountsStore.validateToken(host, token);
    if (ticket !== validationToken) return;
    validation.value = { kind: "valid", result };
  } catch (err) {
    if (ticket !== validationToken) return;
    validation.value = {
      kind: "error",
      message: err instanceof Error ? err.message : "Validation failed.",
    };
  }
}

function handleTokenBlur(): void {
  // Auto-validate only when there's something to validate and we aren't
  // already mid-flight. Re-entry on focus/blur cycles is debounced by the
  // ticket counter in handleValidate.
  if (form.token.trim().length === 0) return;
  if (validation.value.kind === "validating") return;
  void handleValidate();
}

// Any change to host or token invalidates the prior result and cancels any
// in-flight request that hasn't returned yet — so the user can't paste a
// new PAT and have the previous one's result linger in the UI.
watch(
  () => [form.token, form.host],
  () => {
    validationToken++;
    if (validation.value.kind !== "idle") {
      validation.value = { kind: "idle" };
    }
    connectError.value = null;
  },
);

// Auto-switch the tab to match the pasted PAT's prefix. The tab is just a
// help selector; the pasted token's actual type is canonical, so most
// users should land on the matching help by default. Manual tab clicks
// after this fire still work - the `Detected:` pill stays visible to
// keep the truth in front of the user even if they're reading the other
// type's docs.
watch(detectedFlavour, (detected) => {
  if (detected !== null && detected !== form.flavour) {
    form.flavour = detected;
  }
});

async function handleConnect(): Promise<void> {
  submitting.value = true;
  connectError.value = null;
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
    // Token had already validated when Connect was clicked, so don't reset
    // the inline validation status — surface a separate connect-time error
    // near the action row instead.
    connectError.value = err instanceof Error ? err.message : "Could not connect.";
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
  void router.push({ name: "dashboard.authored" });
}

const tokenCreateUrl = computed(() => {
  if (effectiveFlavour.value === "fine-grained") {
    return "https://github.com/settings/personal-access-tokens/new";
  }
  return "https://github.com/settings/tokens/new?scopes=repo,read:org,read:user&description=PRism";
});

interface PermissionRow {
  name: string;
  desc: string;
  access: string;
  required?: boolean;
}

interface PermissionGroup {
  title?: string;
  rows: readonly PermissionRow[];
  orgOnly?: boolean;
}

const fineGrainedGroups: readonly PermissionGroup[] = [
  {
    title: "Repository permissions",
    rows: [
      {
        name: "Contents",
        desc: "Repository contents, commits, branches, downloads, releases, and merges.",
        access: "Read-only",
      },
      {
        name: "Pull requests",
        desc: "Pull requests and related comments, assignees, labels, milestones, and merges.",
        access: "Read-only",
      },
      {
        name: "Metadata",
        desc: "Search repositories, list collaborators, and access repository metadata.",
        access: "Read-only",
        required: true,
      },
    ],
  },
  {
    title: "Organization permissions",
    orgOnly: true,
    rows: [
      {
        name: "Members",
        desc: "Organization members and teams.",
        access: "Read-only",
      },
    ],
  },
];

interface ClassicScope {
  name: string;
  desc: string;
  /** When false, the row shows but Connect doesn't gate on its presence. */
  required?: boolean;
}

const classicScopes: readonly ClassicScope[] = [
  { name: "repo", desc: "Full control of private repositories.", required: true },
  {
    name: "read:org",
    desc: "Read org and team membership, read org projects.",
    required: true,
  },
  { name: "read:user", desc: "Read all user profile data.", required: true },
];

type RowState = "pending" | "granted" | "missing" | "unknown";

function rowStateForScope(scopeName: string): RowState {
  if (validation.value.kind !== "valid") return "pending";
  // GitHub returns scopes verbatim in the x-oauth-scopes header. `repo`
  // also implies the narrower `public_repo`; we accept either as proof of
  // the umbrella scope.
  const scopes = validation.value.result.scopes;
  if (scopes.includes(scopeName)) return "granted";
  if (scopeName === "repo" && scopes.includes("public_repo")) return "granted";
  return "missing";
}

onMounted(() => {
  void syncStore.bind();
  // Skip the welcome step when there's already at least one account -
  // re-entering onboarding from Settings -> Accounts is purely an
  // "add another account" flow; the welcome copy is for first-run only.
  if (accountsStore.accounts.length > 0) {
    currentStep.value = 2;
  }
});

onUnmounted(() => {
  // Leave the sync store bound for other views — it's a singleton.
});
</script>

<template>
  <section class="onboarding">
    <header class="onboarding__header">
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
            <label for="onb-token" class="onboarding-field__label">
              <span>Personal Access Token</span>
              <span
                class="onboarding-field__status"
                :class="`onboarding-field__status--${validation.kind}`"
                role="status"
                aria-live="polite"
              >
                <template v-if="validation.kind === 'idle'">
                  <span class="dot"></span>
                  <span>Validates on blur</span>
                </template>
                <template v-else-if="validation.kind === 'validating'">
                  <span class="onboarding-field__spinner" aria-hidden="true"></span>
                  <span>Validating…</span>
                </template>
                <template v-else-if="validation.kind === 'valid'">
                  <span class="dot dot-success"></span>
                  <span>Token valid · {{ validation.result.login }}</span>
                </template>
                <template v-else>
                  <span class="dot dot-danger"></span>
                  <span>{{ validation.message }}</span>
                </template>
              </span>
            </label>
            <PRismInput
              id="onb-token"
              v-model="form.token"
              size="lg"
              mono
              type="password"
              placeholder="github_pat_… or ghp_…"
              :spellcheck="false"
              autocomplete="off"
              @blur="handleTokenBlur"
            />
            <p
              v-if="detectedFlavour !== null"
              class="onboarding-field__detected"
              :class="{
                'onboarding-field__detected--mismatch':
                  detectedFlavour !== form.flavour,
              }"
            >
              <span class="onboarding-field__detected-dot" aria-hidden="true"></span>
              Detected:
              <strong>
                {{
                  detectedFlavour === "fine-grained"
                    ? "fine-grained PAT"
                    : "classic PAT"
                }}
              </strong>
              <span
                v-if="detectedFlavour !== form.flavour"
                class="onboarding-field__detected-note"
              >
                — Connect uses these requirements, regardless of the tab above.
              </span>
            </p>
            <p class="onboarding-field__hint">
              <a :href="tokenCreateUrl" target="_blank" rel="noreferrer">
                Create a new {{ effectiveFlavour }} PAT
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

          <p
            v-if="form.flavour === 'fine-grained'"
            class="onboarding-scopes__note"
          >
            GitHub doesn't expose granted permissions for fine-grained PATs, so PRism can't
            verify them. Tick the permissions listed below on the PAT page before pasting the
            token.
          </p>

          <template v-if="form.flavour === 'fine-grained'">
            <div
              v-for="group in fineGrainedGroups"
              :key="group.title"
              class="onboarding-scopes__group"
            >
              <header class="onboarding-scopes__group-head">
                <span>{{ group.title }}</span>
                <PRismTooltip v-if="group.orgOnly" side="left" :side-offset="10">
                  <span class="onboarding-scopes__group-hint" tabindex="0">
                    ORG ACCOUNTS ONLY
                  </span>
                  <template #content>
                    <strong class="onboarding-tooltip__title">Only shown for organisation PATs.</strong>
                    Fine-grained tokens only surface an "Organization permissions" section when
                    the Resource owner is an org. Skip this when connecting a personal account.
                  </template>
                </PRismTooltip>
              </header>
              <div
                v-for="row in group.rows"
                :key="row.name"
                class="onboarding-scope"
              >
                <ScopeStateIcon state="info" />
                <div>
                  <div class="onboarding-scope__name">
                    {{ row.name }}
                    <span v-if="row.required" class="onboarding-scope__required">Required</span>
                  </div>
                  <div class="onboarding-scope__desc">{{ row.desc }}</div>
                </div>
                <span class="onboarding-scope__access">{{ row.access }}</span>
              </div>
            </div>
          </template>

          <template v-else>
            <div
              v-for="scope in classicScopes"
              :key="scope.name"
              class="onboarding-scope"
              :class="`onboarding-scope--${rowStateForScope(scope.name)}`"
            >
              <ScopeStateIcon :state="rowStateForScope(scope.name)" />
              <div>
                <div class="onboarding-scope__name onboarding-scope__name--scope">{{ scope.name }}</div>
                <div class="onboarding-scope__desc">{{ scope.desc }}</div>
              </div>
              <ScopeStateTag :state="rowStateForScope(scope.name)" />
            </div>
          </template>
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

        <div v-if="connectError" class="onboarding-message onboarding-message--error">
          {{ connectError }}
        </div>

        <div class="onboarding-step__foot">
          <PRismButton @click="goTo(1)">Back</PRismButton>
          <PRismButton variant="primary" size="lg" :disabled="!canConnect" @click="handleConnect">
            <span v-if="submitting">Connecting…</span>
            <span v-else>Connect</span>
          </PRismButton>
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
  justify-content: center;
  padding: var(--s-5) var(--s-8);
  border-bottom: 1px solid var(--border-1);
  background: var(--bg-1);
  flex: 0 0 auto;
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
  /* Don't stretch the step to the body's height — its column-flex children
   * would then shrink to fit and clip overflow:hidden blocks like
   * `.onboarding-scopes`. Letting the step take its content height means
   * the body's overflow:auto scrolls the whole thing instead. */
  align-items: flex-start;
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
  display: flex;
  align-items: center;
  gap: var(--s-3);
}

.onboarding-field__status {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-family: var(--font-sans);
  font-size: var(--fs-11);
  text-transform: none;
  letter-spacing: 0;
  color: var(--text-mute);
}

.onboarding-field__status--valid {
  color: var(--success);
}

.onboarding-field__status--error {
  color: var(--danger);
}

.onboarding-field__spinner {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  border: 1.5px solid var(--border-3);
  border-top-color: var(--accent);
  animation: onboarding-spinner 0.8s linear infinite;
}

@keyframes onboarding-spinner {
  to { transform: rotate(360deg); }
}

.onboarding-field__hint {
  margin: 2px 0 0;
  color: var(--text-mute);
  font-size: var(--fs-12);
}

.onboarding-field__detected {
  margin: 4px 0 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.onboarding-field__detected strong {
  color: var(--text);
  font-weight: 600;
}

.onboarding-field__detected-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--info);
  flex: 0 0 6px;
}

.onboarding-field__detected--mismatch .onboarding-field__detected-dot {
  background: var(--warning);
}

.onboarding-field__detected-note {
  color: var(--warning);
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

.onboarding-scope__name {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text);
}

.onboarding-scope__desc {
  color: var(--text-mute);
  font-size: var(--fs-11);
}

.onboarding-scope__name--scope {
  color: var(--accent-strong);
}

.onboarding-scope__access {
  font-family: var(--font-mono);
  font-size: var(--fs-9);
  color: var(--text-faint);
  text-transform: uppercase;
  letter-spacing: 0.6px;
  padding: 1px 6px;
  background: var(--bg-4);
  border-radius: var(--r-1);
}

.onboarding-scopes__note {
  margin: 0;
  padding: 10px 14px;
  font-size: var(--fs-11);
  color: var(--text-mute);
  background: var(--info-bg);
  border-bottom: 1px solid var(--border-1);
}

.onboarding-scope__required {
  font-family: var(--font-sans);
  font-size: var(--fs-9);
  color: var(--warning);
  background: var(--warning-bg);
  text-transform: uppercase;
  letter-spacing: 0.6px;
  padding: 1px 5px;
  border-radius: var(--r-1);
  margin-left: 6px;
  vertical-align: 1px;
}

.onboarding-scopes__group {
  border-top: 1px solid var(--border-1);
}

.onboarding-scopes__group:first-of-type {
  border-top: 0;
}

.onboarding-scopes__group-head {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  padding: 8px 14px 4px;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-mute);
  text-transform: uppercase;
  letter-spacing: 0.8px;
}

.onboarding-scopes__group-hint {
  font-family: var(--font-mono);
  font-size: var(--fs-9);
  color: var(--info);
  background: var(--info-bg);
  text-transform: uppercase;
  letter-spacing: 0.6px;
  padding: 1px 6px;
  border-radius: var(--r-1);
  outline: none;
  margin-left: auto;
}

.onboarding-scopes__group-hint:focus-visible {
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.onboarding-tooltip__title {
  display: block;
  color: var(--text-strong);
  font-weight: 600;
  margin-bottom: 2px;
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
