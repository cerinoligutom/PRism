import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

export interface Account {
  readonly id: number;
  readonly label: string;
  readonly host: string;
  readonly login: string;
  readonly scopes: readonly string[];
  readonly expires_at: string | null;
  /**
   * GitHub avatar URL for `login`, resolved at read time by the backend via
   * `LEFT JOIN users` (ADR 0013). `null` when no sync cycle has yet populated
   * the `users` row for this login, in which case the UI falls back to the
   * palette swatch.
   */
  readonly avatar_url: string | null;
}

export type PermissionState = "granted" | "missing" | "unknown";

export interface PermissionChecks {
  readonly contents: PermissionState;
  readonly pull_requests: PermissionState;
  readonly metadata: PermissionState;
  readonly members: PermissionState;
}

export interface ValidateTokenResult {
  readonly login: string;
  readonly scopes: readonly string[];
  readonly expires_at: string | null;
  readonly permissions: PermissionChecks;
}

export interface AddAccountInput {
  readonly label: string;
  readonly host: string;
  readonly token: string;
}

type AuthCommandError =
  | { kind: "unauthorized" }
  | { kind: "forbidden" }
  | { kind: "network"; host: string }
  | { kind: "not_found" }
  | { kind: "internal" };

/**
 * Translates the structured Rust error into a single user-facing message.
 * The shape comes from `#[serde(tag = "kind", rename_all = "snake_case")]`
 * on `AuthCommandError` in `src-tauri/src/auth/commands.rs`.
 */
function formatAuthError(raw: unknown): string {
  if (typeof raw === "object" && raw !== null && "kind" in raw) {
    const err = raw as AuthCommandError;
    switch (err.kind) {
      case "unauthorized":
        return "GitHub rejected the token. Check that it hasn't expired or been revoked.";
      case "forbidden":
        return "Token is missing one of the required permissions.";
      case "network":
        return `Couldn't reach ${err.host}. Check your connection or the host name.`;
      case "not_found":
        return "Account not found.";
      case "internal":
        return "Something went wrong saving the account. Check the application logs.";
    }
  }
  return typeof raw === "string" ? raw : "Unexpected error.";
}

export const useAccountsStore = defineStore("accounts", () => {
  const accounts = ref<Account[]>([]);
  const loading = ref(false);
  const lastError = ref<string | null>(null);

  const isEmpty = computed(() => accounts.value.length === 0);
  const count = computed(() => accounts.value.length);

  async function refresh(): Promise<void> {
    loading.value = true;
    lastError.value = null;
    try {
      accounts.value = await invoke<Account[]>("list_accounts");
    } catch (err) {
      lastError.value = formatAuthError(err);
    } finally {
      loading.value = false;
    }
  }

  async function validateToken(host: string, token: string): Promise<ValidateTokenResult> {
    try {
      return await invoke<ValidateTokenResult>("validate_token_cmd", {
        input: { host, token },
      });
    } catch (err) {
      throw new Error(formatAuthError(err));
    }
  }

  async function addAccount(input: AddAccountInput): Promise<Account> {
    loading.value = true;
    lastError.value = null;
    try {
      const account = await invoke<Account>("add_account", { input });
      accounts.value = [...accounts.value, account];
      return account;
    } catch (err) {
      const message = formatAuthError(err);
      lastError.value = message;
      throw new Error(message);
    } finally {
      loading.value = false;
    }
  }

  async function removeAccount(id: number): Promise<void> {
    loading.value = true;
    lastError.value = null;
    try {
      await invoke<void>("remove_account", { id });
      accounts.value = accounts.value.filter((a) => a.id !== id);
    } catch (err) {
      lastError.value = formatAuthError(err);
      throw new Error(lastError.value);
    } finally {
      loading.value = false;
    }
  }

  function clearError(): void {
    lastError.value = null;
  }

  return {
    accounts,
    loading,
    lastError,
    isEmpty,
    count,
    refresh,
    validateToken,
    addAccount,
    removeAccount,
    clearError,
  };
});
