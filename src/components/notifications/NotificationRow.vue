<script setup lang="ts">
import PRismBadge from "@/components/ui/PRismBadge.vue";
import PRismButton from "@/components/ui/PRismButton.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import type { Notification } from "@/stores/notifications";

interface Props {
  notification: Notification;
}

defineProps<Props>();

const emit = defineEmits<{
  open: [notification: Notification];
  delete: [notification: Notification];
}>();

function onOpen(notification: Notification): void {
  emit("open", notification);
}

function onDelete(event: Event, notification: Notification): void {
  // Stop propagation so the row's primary click handler doesn't fire the
  // open intent at the same time as the delete.
  event.stopPropagation();
  emit("delete", notification);
}

function onKeydown(event: KeyboardEvent, notification: Notification): void {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    emit("open", notification);
  }
}

function kindLabel(kind: string): string {
  if (kind === "needs_attention") return "Needs attention";
  if (kind === "mention") return "Mention";
  return kind;
}
</script>

<template>
  <article
    class="notification-row"
    :class="{ 'notification-row--unread': notification.read_at === null }"
    role="button"
    tabindex="0"
    @click="onOpen(notification)"
    @keydown="onKeydown($event, notification)"
  >
    <span class="notification-row__icon" aria-hidden="true">
      <!-- Bell icon. Same affordance for both trigger kinds at the row
           level - the badge below names the kind. -->
      <svg
        width="16"
        height="16"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="M3.5 6.5a4.5 4.5 0 019 0v3l1 2H2.5l1-2z" />
        <path d="M6.5 13.5a1.5 1.5 0 003 0" />
      </svg>
    </span>

    <div class="notification-row__body">
      <div class="notification-row__head">
        <PRismBadge :variant="notification.kind === 'mention' ? 'accent' : 'info'">
          {{ kindLabel(notification.kind) }}
        </PRismBadge>
        <span class="repo-chip">
          <span class="org">{{ notification.owner }}</span>
          <span class="slash">/</span>
          <span class="repo">{{ notification.repo }}</span>
          <span class="text-fg-faint">#{{ notification.pr_number }}</span>
        </span>
        <PRismRelativeTime
          class="notification-row__time"
          :value="notification.created_at"
        />
      </div>
      <p class="notification-row__title">{{ notification.title }}</p>
      <p v-if="notification.body" class="notification-row__snippet">
        {{ notification.body }}
      </p>
    </div>

    <div class="notification-row__actions">
      <PRismTooltip text="Dismiss">
        <PRismButton
          variant="ghost"
          size="sm"
          :icon="true"
          aria-label="Dismiss notification"
          @click="onDelete($event, notification)"
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-linecap="round"
          >
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </PRismButton>
      </PRismTooltip>
    </div>
  </article>
</template>

<style scoped>
.notification-row {
  display: grid;
  grid-template-columns: auto 1fr auto;
  align-items: flex-start;
  gap: var(--s-3);
  padding: 12px 16px;
  background: var(--bg-2);
  border: 1px solid var(--border-1);
  border-left-width: 3px;
  border-radius: var(--r-2);
  cursor: pointer;
  text-align: left;
}

.notification-row:hover {
  background: var(--bg-3);
  border-color: var(--border-2);
}

.notification-row--unread {
  border-left-color: var(--accent);
}

.notification-row--unread:hover {
  border-left-color: var(--accent);
}

.notification-row:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.notification-row__icon {
  color: var(--text-mute);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  margin-top: 1px;
}

.notification-row__body {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.notification-row__head {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
}

.notification-row__time {
  margin-left: auto;
  font-size: var(--fs-11);
  color: var(--text-faint);
}

.notification-row__title {
  margin: 0;
  font-size: var(--fs-13);
  font-weight: 500;
  color: var(--text-strong);
}

.notification-row__snippet {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}

.notification-row__actions {
  display: flex;
  align-items: center;
  gap: var(--s-1);
}
</style>
