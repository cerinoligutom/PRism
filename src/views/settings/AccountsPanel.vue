<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import PRismButton from "@/components/ui/PRismButton.vue";
import PRismCallout from "@/components/ui/PRismCallout.vue";
import ReauthDialog from "./ReauthDialog.vue";
import RemoveAccountDialog from "./RemoveAccountDialog.vue";
import { useAccountsStore, type Account } from "@/stores/accounts";

const accountsStore = useAccountsStore();

// Per-account re-auth (issue #59): when non-null, the `ReauthDialog` opens
// targeting this account. Triggered from the card's Re-auth button and from
// the danger banner's Re-authenticate action; closes via Cancel / Esc /
// overlay click / successful submit.
const reauthTarget = ref<Account | null>(null);

// Per-account remove confirmation (issue #386): `window.confirm` is unreliable
// in the Tauri 2 webview, so the Remove action drives a Reka-based dialog
// mirroring the re-auth flow.
const removeTarget = ref<Account | null>(null);

interface ReauthRequiredPayload {
  readonly account_id: number;
  readonly label: string;
}

const expiredAccountIds = ref<Set<number>>(new Set());
let unlistenReauth: UnlistenFn | null = null;

// Tracks per-account `<img>` load failures (404 / stale URL / offline) so the
// initials swatch takes over on the next render. Keyed on `account.id` rather
// than URL so a refreshed avatar via the next sync cycle resets cleanly when
// the row is re-rendered.
const imageFailures = reactive<Set<number>>(new Set());

const expiredAccounts = computed(() =>
  accountsStore.accounts.filter((a) => expiredAccountIds.value.has(a.id))
);

const sublabel = computed(() => {
  const n = accountsStore.count;
  return `${n} ACCOUNT${n === 1 ? "" : "S"}`;
});

function paletteClass(id: number): string {
  // Cycle the seeded palette swatches from primitives.css so each account
  // gets a stable but visually distinct mark when the real avatar is absent.
  const slot = ((id - 1) % 9) + 1;
  return `av-${slot}`;
}

function showAvatarImage(account: Account): boolean {
  return (
    typeof account.avatar_url === "string"
    && account.avatar_url.length > 0
    && !imageFailures.has(account.id)
  );
}

function onAvatarError(account: Account): void {
  imageFailures.add(account.id);
}

function initials(account: Account): string {
  const source = account.label || account.login;
  return source
    .split(/[\s\-_/]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase() ?? "")
    .join("") || "?";
}

function tokenStatus(account: Account): {
  state: "valid" | "expiring" | "expired";
  daysRemaining: number | null;
} {
  if (expiredAccountIds.value.has(account.id)) {
    return { state: "expired", daysRemaining: null };
  }
  if (!account.expires_at) {
    return { state: "valid", daysRemaining: null };
  }
  const expiry = new Date(account.expires_at);
  if (Number.isNaN(expiry.getTime())) {
    return { state: "valid", daysRemaining: null };
  }
  const now = Date.now();
  const days = Math.round((expiry.getTime() - now) / (1000 * 60 * 60 * 24));
  if (days <= 0) {
    return { state: "expired", daysRemaining: days };
  }
  if (days <= 14) {
    return { state: "expiring", daysRemaining: days };
  }
  return { state: "valid", daysRemaining: days };
}

function openRemove(account: Account): void {
  removeTarget.value = account;
}

function closeRemove(): void {
  removeTarget.value = null;
}

async function confirmRemove(): Promise<void> {
  const account = removeTarget.value;
  if (account === null) return;
  try {
    // Schema cascades on `accounts.id` deletion (init.sql, 0002), so the
    // user's per-account read / archive state in `pull_request_viewer_relations`
    // is dropped alongside the PAT.
    await accountsStore.removeAccount(account.id);
    expiredAccountIds.value.delete(account.id);
  } catch {
    // Store has already populated lastError for the banner.
  } finally {
    removeTarget.value = null;
  }
}

function dismissReauth(account: Account): void {
  expiredAccountIds.value.delete(account.id);
}

function openReauth(account: Account): void {
  reauthTarget.value = account;
}

function closeReauth(): void {
  reauthTarget.value = null;
}

function onReauthSuccess(accountId: number): void {
  // Clear any expired-marker the banner relied on so the success state is
  // visible on the next render; the worker nudge handles waking the parked
  // sync cycle. If the new PAT itself 401s on the next cycle the banner
  // re-appears via the `auth://reauth-required` event.
  expiredAccountIds.value.delete(accountId);
}

onMounted(async () => {
  await accountsStore.refresh();
  try {
    unlistenReauth = await listen<ReauthRequiredPayload>("auth://reauth-required", (event) => {
      expiredAccountIds.value.add(event.payload.account_id);
    });
  } catch {
    // Tauri event bus unavailable (e.g. running outside the desktop shell).
    unlistenReauth = null;
  }
});

onUnmounted(() => {
  unlistenReauth?.();
});
</script>

<template>
  <div class="accounts-panel">
    <header class="accounts-panel__header">
      <h1 class="accounts-panel__title">Accounts</h1>
      <span class="accounts-panel__sub">{{ sublabel }}</span>
    </header>

    <PRismCallout
      v-for="account in expiredAccounts"
      :key="`banner-${account.id}`"
      variant="danger"
      class="accounts-panel__banner"
    >
      <template #icon>
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M8 3v6M8 11v.5" />
          <circle cx="8" cy="8" r="6.5" />
        </svg>
      </template>
      <div class="accounts-panel__banner-body">
        <div>
          <div class="accounts-panel__banner-title">
            The token for <strong>{{ account.label }}</strong> needs re-authentication.
          </div>
          <div class="accounts-panel__banner-desc">
            PRism stopped syncing PRs from this account. Re-add the PAT to resume.
          </div>
        </div>
        <div class="accounts-panel__banner-actions">
          <PRismButton @click="dismissReauth(account)">Dismiss</PRismButton>
          <PRismButton variant="primary" @click="openReauth(account)">
            Re-authenticate
          </PRismButton>
        </div>
      </div>
    </PRismCallout>

    <section class="accounts-panel__section">
      <div class="accounts-panel__section-head">
        <div class="accounts-panel__section-head-text">
          <h3 class="accounts-panel__section-title">GitHub accounts</h3>
          <span class="accounts-panel__section-desc">
            PATs are stored exclusively in the OS keychain. PRism never writes them to disk or logs.
          </span>
        </div>
      </div>

      <div v-if="accountsStore.loading && accountsStore.isEmpty" class="accounts-panel__loading">
        Loading accounts…
      </div>

      <div v-else-if="accountsStore.isEmpty" class="accounts-panel__empty">
        <p class="accounts-panel__empty-copy">No accounts connected yet.</p>
        <PRismButton to="/onboarding" variant="primary">Connect an account</PRismButton>
      </div>

      <div v-else class="accounts-panel__list">
        <article
          v-for="account in accountsStore.accounts"
          :key="account.id"
          class="account-card"
          :class="{ 'account-card--expired': expiredAccountIds.has(account.id) }"
        >
          <span
            class="avatar account-card__avatar"
            :class="[showAvatarImage(account) ? 'account-card__avatar--image' : paletteClass(account.id)]"
          >
            <img
              v-if="showAvatarImage(account)"
              :src="account.avatar_url ?? undefined"
              :alt="account.login || account.label"
              class="account-card__avatar-img"
              loading="lazy"
              decoding="async"
              @error="onAvatarError(account)"
            />
            <template v-else>{{ initials(account) }}</template>
          </span>
          <div class="account-card__info">
            <div class="account-card__label">
              {{ account.label || account.login }}
              <span class="account-card__host">{{ account.host }}</span>
            </div>
            <div class="account-card__sub">
              <template v-if="tokenStatus(account).state === 'expired'">
                <span class="account-card__sub-err">Token needs re-auth</span>
              </template>
              <template v-else>
                <span class="account-card__sub-ok">Token valid</span>
              </template>
              <span class="account-card__sep">·</span>
              <span class="account-card__login">{{ account.login }}</span>
              <template v-if="account.scopes.length > 0">
                <span class="account-card__sep">·</span>
                <span class="account-card__scopes">{{ account.scopes.join(", ") }}</span>
              </template>
              <template v-if="account.expires_at">
                <span class="account-card__sep">·</span>
                <span>expires&nbsp;</span>
                <strong
                  :class="{
                    'account-card__warning': tokenStatus(account).state === 'expiring',
                    'account-card__danger': tokenStatus(account).state === 'expired',
                  }"
                >
                  <template v-if="tokenStatus(account).daysRemaining === null">
                    {{ account.expires_at }}
                  </template>
                  <template v-else-if="tokenStatus(account).daysRemaining! > 0">
                    in {{ tokenStatus(account).daysRemaining }} days
                  </template>
                  <template v-else>
                    {{ -tokenStatus(account).daysRemaining! }} days ago
                  </template>
                </strong>
              </template>
            </div>
          </div>
          <div class="account-card__actions">
            <PRismButton size="sm" @click="openRemove(account)">Remove</PRismButton>
            <PRismButton size="sm" variant="primary" @click="openReauth(account)">
              Re-auth
            </PRismButton>
          </div>
        </article>

      </div>

      <div v-if="!accountsStore.isEmpty" class="accounts-panel__list-footer">
        <PRismButton to="/onboarding" variant="primary" size="sm">
          + Add account
        </PRismButton>
      </div>

      <div v-if="accountsStore.lastError" class="accounts-panel__error">
        {{ accountsStore.lastError }}
      </div>
    </section>

    <ReauthDialog
      :account="reauthTarget"
      @close="closeReauth"
      @success="onReauthSuccess"
    />
    <RemoveAccountDialog
      :account="removeTarget"
      @close="closeRemove"
      @confirm="confirmRemove"
    />
  </div>
</template>

<style scoped>
.accounts-panel__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  margin-bottom: var(--s-6);
}

.accounts-panel__title {
  margin: 0;
  font-size: var(--fs-24);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}

.accounts-panel__sub {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  letter-spacing: 0.5px;
}

.accounts-panel__banner {
  margin-bottom: var(--s-6);
}

.accounts-panel__banner-body {
  display: flex;
  align-items: center;
  gap: var(--s-4);
  width: 100%;
}

.accounts-panel__banner-title {
  font-size: var(--fs-13);
  font-weight: 600;
  color: var(--text-strong);
}

.accounts-panel__banner-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
  margin-top: 2px;
}

.accounts-panel__banner-actions {
  display: flex;
  gap: var(--s-2);
  margin-left: auto;
  flex-shrink: 0;
}

.accounts-panel__section {
  margin-bottom: var(--s-7);
}

.accounts-panel__section-head {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: var(--s-4);
}

.accounts-panel__section-head-text {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  min-width: 0;
}

.accounts-panel__section-title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
}

.accounts-panel__section-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.accounts-panel__loading {
  padding: var(--s-5);
  color: var(--text-mute);
  font-size: var(--fs-12);
}

.accounts-panel__empty {
  padding: var(--s-7) var(--s-6);
  background: var(--bg-2);
  border: 1px dashed var(--border-2);
  border-radius: var(--r-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  align-items: center;
}

.accounts-panel__empty-copy {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
}

.accounts-panel__list {
  display: flex;
  flex-direction: column;
  gap: 1px;
  background: var(--border-1);
  border-radius: var(--r-3);
  overflow: hidden;
}

.accounts-panel__list-footer {
  margin-top: var(--s-4);
  display: flex;
  justify-content: flex-end;
}

.accounts-panel__error {
  margin-top: var(--s-4);
  padding: 10px 14px;
  border-radius: var(--r-2);
  background: var(--danger-bg);
  color: var(--danger);
  font-size: var(--fs-12);
}

/* ────── account-card BEM block ────── */
.account-card {
  background: var(--bg-2);
  padding: 14px 18px;
  display: grid;
  grid-template-columns: 36px 1fr auto;
  gap: var(--s-4);
  align-items: center;
}

.account-card--expired {
  background: oklch(0.18 0.05 25 / 0.3);
}

.account-card__avatar {
  width: 36px;
  height: 36px;
  font-size: var(--fs-12);
  border-radius: 8px;
}

/* Real GitHub avatar layered on top of the .avatar primitive. Container loses
 * the palette colour but keeps the same box so swapping image <-> initials
 * doesn't move surrounding content. The padding reset lets the <img> reach
 * the rounded edge; `overflow: hidden` clips it to the border-radius. */
.account-card__avatar--image {
  background: var(--bg-4);
  padding: 0;
  overflow: hidden;
}

.account-card__avatar-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.account-card__label {
  font-size: var(--fs-13);
  font-weight: 600;
  color: var(--text-strong);
  display: flex;
  align-items: center;
  gap: 6px;
}

.account-card__host {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  font-weight: 400;
  padding: 1px 5px;
  background: var(--bg-3);
  border-radius: var(--r-1);
  letter-spacing: 0.3px;
}

.account-card__sub {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-mute);
  margin-top: 3px;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 2px;
}

.account-card__sub-ok {
  color: var(--success);
}

.account-card__sub-ok::before,
.account-card__sub-err::before {
  content: "● ";
}

.account-card__sub-err {
  color: var(--danger);
}

.account-card__sep {
  color: var(--text-disabled);
  margin: 0 4px;
}

.account-card__login {
  color: var(--text-mute);
}

.account-card__scopes {
  color: var(--text-mute);
}

.account-card__warning {
  color: var(--warning);
}

.account-card__danger {
  color: var(--danger);
}

.account-card__actions {
  display: flex;
  gap: 6px;
}
</style>
