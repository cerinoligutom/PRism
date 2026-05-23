<script setup lang="ts">
import { computed } from "vue";
import {
  PopoverContent,
  PopoverPortal,
  PopoverRoot,
  PopoverTrigger,
} from "reka-ui";

import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismAvatarStack from "@/components/ui/PRismAvatarStack.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import type { Account } from "@/stores/accounts";

/**
 * Dashboard account-scope picker. Drives `dashboard.accountScope` and
 * persists the choice in `appearance.accountScope` (ADR 0016, "Account
 * picker - option 1"). Renders the unified "All accounts" entry at the
 * top followed by one row per tracked account.
 *
 * Single-account special case: with exactly one tracked account, the
 * trigger renders disabled showing the account label - keeps the affordance
 * visible without offering a non-choice.
 */
interface Props {
  /** Tracked accounts, in `accounts.refresh()` order. */
  accounts: readonly Account[];
  /** Selected scope. `null` = unified ("All accounts"). */
  modelValue: number | null;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  "update:modelValue": [value: number | null];
}>();

const isSingleAccount = computed<boolean>(() => props.accounts.length === 1);

const selectedAccount = computed<Account | null>(() => {
  if (props.modelValue === null) return null;
  return props.accounts.find((a) => a.id === props.modelValue) ?? null;
});

const soloAccount = computed<Account | null>(() =>
  isSingleAccount.value ? (props.accounts[0] ?? null) : null,
);

function accountDisplay(account: Account): string {
  return account.label || account.login;
}

const triggerLabel = computed<string>(() => {
  if (selectedAccount.value !== null) return accountDisplay(selectedAccount.value);
  if (soloAccount.value !== null) return accountDisplay(soloAccount.value);
  return "All accounts";
});

// Up to three avatars feed the stacked mark for the "All accounts" trigger.
// Keeps the chip terse while still hinting at the union; the dropdown lists
// every account.
const stackUsers = computed<readonly { login: string; avatar_url: string | null }[]>(() =>
  props.accounts.slice(0, 3).map((a) => ({
    login: a.login,
    avatar_url: a.avatar_url,
  })),
);

// Only the disabled single-account variant uses a tooltip - the interactive
// popover trigger has the label inline and a caret, so the affordance is
// self-explanatory. The disabled trigger benefits from a hover hint that
// names why it's locked.
const soloTooltipText = computed<string>(() => {
  const only = soloAccount.value;
  return only === null ? "" : `Scoped to ${accountDisplay(only)} (single account)`;
});

function isNonGithub(host: string): boolean {
  return host.toLowerCase() !== "github.com";
}

function selectUnified(): void {
  if (props.modelValue !== null) emit("update:modelValue", null);
}

function selectAccount(account: Account): void {
  if (props.modelValue !== account.id) emit("update:modelValue", account.id);
}
</script>

<template>
  <PopoverRoot v-if="!isSingleAccount">
    <PopoverTrigger as-child>
      <button
        type="button"
        class="account-picker__trigger"
        :aria-label="`Account scope: ${triggerLabel}`"
      >
        <span class="account-picker__mark" aria-hidden="true">
          <template v-if="selectedAccount !== null">
            <PRismAvatar
              :login="selectedAccount.login"
              :avatar-url="selectedAccount.avatar_url"
              size="sm"
              :title="null"
            />
          </template>
          <template v-else-if="accounts.length > 0">
            <PRismAvatarStack
              :users="stackUsers"
              size="sm"
              layout="overlap"
            />
          </template>
          <template v-else>
            <svg
              width="14"
              height="14"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.4"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <circle cx="6" cy="6" r="2.5" />
              <circle cx="10" cy="6" r="2.5" />
              <path d="M2.5 13c0-1.7 1.7-3 3.5-3M13.5 13c0-1.7-1.7-3-3.5-3" />
            </svg>
          </template>
        </span>
        <span class="account-picker__label">{{ triggerLabel }}</span>
        <svg
          class="account-picker__caret"
          width="9"
          height="9"
          viewBox="0 0 9 9"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M2 3.5l2.5 2.5L7 3.5" />
        </svg>
      </button>
    </PopoverTrigger>

    <PopoverPortal>
      <PopoverContent
        class="account-picker__menu"
        align="start"
        :side-offset="6"
        :collision-padding="12"
      >
        <button
          type="button"
          class="account-picker__option"
          :class="{ 'account-picker__option--active': modelValue === null }"
          :aria-selected="modelValue === null"
          @click="selectUnified"
        >
          <span class="account-picker__option-mark" aria-hidden="true">
            <PRismAvatarStack
              v-if="accounts.length > 0"
              :users="stackUsers"
              size="sm"
              layout="overlap"
            />
            <svg
              v-else
              width="14"
              height="14"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.4"
            >
              <circle cx="6" cy="6" r="2.5" />
              <circle cx="10" cy="6" r="2.5" />
            </svg>
          </span>
          <span class="account-picker__option-body">
            <span class="account-picker__option-label">All accounts</span>
            <span class="account-picker__option-sub">Unified across every tracked account</span>
          </span>
          <span
            v-if="modelValue === null"
            class="account-picker__option-tick"
            aria-hidden="true"
          >
            <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3.5 8.5l3 3 6-6.5" />
            </svg>
          </span>
        </button>

        <div class="account-picker__sep" role="separator" aria-hidden="true" />

        <button
          v-for="account in accounts"
          :key="account.id"
          type="button"
          class="account-picker__option"
          :class="{ 'account-picker__option--active': modelValue === account.id }"
          :aria-selected="modelValue === account.id"
          @click="selectAccount(account)"
        >
          <span class="account-picker__option-mark" aria-hidden="true">
            <PRismAvatar
              :login="account.login"
              :avatar-url="account.avatar_url"
              size="sm"
              :title="null"
            />
          </span>
          <span class="account-picker__option-body">
            <span class="account-picker__option-label">
              {{ accountDisplay(account) }}
              <span
                v-if="isNonGithub(account.host)"
                class="account-picker__option-host"
              >{{ account.host }}</span>
            </span>
            <span class="account-picker__option-sub">{{ account.login }}</span>
          </span>
          <span
            v-if="modelValue === account.id"
            class="account-picker__option-tick"
            aria-hidden="true"
          >
            <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3.5 8.5l3 3 6-6.5" />
            </svg>
          </span>
        </button>
      </PopoverContent>
    </PopoverPortal>
  </PopoverRoot>

  <PRismTooltip v-else-if="soloAccount !== null" :text="soloTooltipText" :as-child="true">
    <button
      type="button"
      class="account-picker__trigger account-picker__trigger--solo"
      :disabled="true"
      aria-label="Dashboard scope (single account)"
    >
      <span class="account-picker__mark" aria-hidden="true">
        <PRismAvatar
          :login="soloAccount.login"
          :avatar-url="soloAccount.avatar_url"
          size="sm"
          :title="null"
        />
      </span>
      <span class="account-picker__label">{{ triggerLabel }}</span>
    </button>
  </PRismTooltip>
</template>

<style scoped>
.account-picker__trigger {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 26px;
  padding: 0 10px 0 6px;
  background: var(--bg-2);
  border: 1px solid var(--border-2);
  border-radius: var(--r-2);
  color: var(--text);
  font-size: var(--fs-12);
  font-weight: 500;
  cursor: pointer;
  transition: background 0.12s, border-color 0.12s, color 0.12s;
}

.account-picker__trigger:hover:not(:disabled) {
  background: var(--bg-3);
  border-color: var(--border-3);
}

.account-picker__trigger:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.account-picker__trigger[data-state="open"] {
  background: var(--bg-3);
  border-color: var(--border-3);
}

.account-picker__trigger--solo {
  cursor: default;
  color: var(--text-mute);
}

.account-picker__trigger--solo:disabled {
  opacity: 1;
}

.account-picker__mark {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 16px;
  color: var(--text-mute);
}

.account-picker__label {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 160px;
}

.account-picker__caret {
  color: var(--text-faint);
  flex: 0 0 auto;
}
</style>

<!--
  Popover content is teleported to `document.body` via Reka's `PopoverPortal`.
  Scoped styles can't follow the teleport, so the menu rules live in an
  unscoped block. Class names are BEM-namespaced (`account-picker__*`) so they
  don't collide with other menu/option surfaces.
-->
<style>
.account-picker__menu {
  min-width: 240px;
  max-width: 320px;
  background: var(--bg-2);
  border: 1px solid var(--border-2);
  border-radius: var(--r-2);
  padding: 4px;
  box-shadow: var(--shadow-2);
  z-index: 60;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.account-picker__option {
  display: grid;
  grid-template-columns: 20px 1fr auto;
  align-items: center;
  gap: 10px;
  padding: 6px 8px;
  background: transparent;
  border: 0;
  border-radius: var(--r-1);
  color: var(--text);
  text-align: left;
  cursor: pointer;
  font: inherit;
}

.account-picker__option:hover,
.account-picker__option:focus-visible {
  background: var(--bg-4);
  outline: none;
}

.account-picker__option--active {
  background: var(--accent-bg);
  color: var(--accent-strong);
}

.account-picker__option-mark {
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.account-picker__option-body {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.account-picker__option-label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-12);
  font-weight: 500;
  color: var(--text-strong);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.account-picker__option--active .account-picker__option-label {
  color: var(--accent-strong);
}

.account-picker__option-host {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  font-weight: 400;
  color: var(--text-faint);
  padding: 1px 5px;
  background: var(--bg-3);
  border-radius: var(--r-1);
  letter-spacing: 0.3px;
}

.account-picker__option-sub {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.account-picker__option-tick {
  color: var(--accent);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.account-picker__sep {
  height: 1px;
  background: var(--border-1);
  margin: 4px 0;
}
</style>
