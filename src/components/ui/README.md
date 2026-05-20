# UI primitives

This is the **third layer** of PRism's component stack — typed Vue wrappers around the CSS primitives in [`../../assets/styles/primitives.css`](../../assets/styles/primitives.css) and Reka UI's headless behaviour primitives.

See the "Components — three-layer primitives stack" section in [`CLAUDE.md`](../../../CLAUDE.md) for the rule.

## What lives here

Components named `PRism*` (e.g. `PRismButton.vue`, `PRismBadge.vue`, `PRismDialog.vue`) that wrap the lower layers with:

- A typed `<script setup lang="ts">` API using `defineProps<{ variant?: ... }>()`.
- Explicit union types for variants (`"default" | "primary" | "ghost"`), not `string`.
- `withDefaults` for default values.
- Where applicable, a `to` / `href` prop that switches the rendered element so call sites stay flat.

## What does NOT live here

- App-level components (`AppShell`, `SidebarNav`, `StatusBar`) — they sit one layer up in `src/components/`.
- View components — they live in `src/views/`.
- One-off layout helpers used by exactly one parent — those are scoped CSS inside the parent.

## When to add a primitive

When the same pattern is about to appear in **three** places. Two is borderline; one is premature. The point is centralising shared behaviour, not pre-building every conceivable component.

Concrete trigger: you're about to copy-paste `class="btn btn-primary btn-lg"` for a third time, or you're adding a third `RouterLink` styled as a button. Extract the primitive in the same PR as the third call site.
