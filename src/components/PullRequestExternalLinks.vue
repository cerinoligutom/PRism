<script setup lang="ts">
import { computed } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

interface Props {
  owner: string;
  repo: string;
  number: number;
  url: string;
}

const props = defineProps<Props>();

// Unravel mirrors GitHub's URL shape (owner/repo/pull/N) on the unravel.sh
// host, so the same fields the GitHub link uses build the Unravel one.
const unravelUrl = computed<string>(
  () =>
    `https://www.unravel.sh/${props.owner}/${props.repo}/pull/${props.number}`,
);
</script>

<template>
  <div class="pr-ext-links">
    <PRismTooltip text="Open on Unravel" :as-child="true">
      <button
        type="button"
        class="pr-ext-links__btn"
        aria-label="Open on Unravel"
        @click="openUrl(unravelUrl)"
      >
        <svg
          width="14"
          height="14"
          viewBox="287 261 441 447"
          fill="currentColor"
          aria-hidden="true"
        >
          <g
            transform="translate(0 1024) scale(0.1 -0.1)"
            fill="currentColor"
            stroke="none"
          >
            <path
              d="M4755 7599 c-251 -37 -444 -98 -680 -214 -105 -51 -215 -115 -288 -165 -128 -90 -305 -253 -415 -383 -249 -293 -436 -707 -483 -1067 -17 -130 -17 -551 -1 -665 52 -349 215 -725 439 -1009 78 -98 231 -254 336 -342 518 -434 1260 -593 1959 -418 693 173 1263 680 1511 1344 44 118 102 344 122 479 20 131 20 424 0 574 -50 380 -215 752 -481 1087 -331 415 -869 713 -1419 785 -163 21 -435 18 -600 -6z m465 -350 c55 -6 113 -15 128 -19 l27 -8 -27 -1 c-57 -2 -236 -42 -343 -77 -239 -77 -437 -196 -606 -364 -239 -236 -372 -481 -440 -810 -29 -140 -31 -425 -5 -575 43 -239 152 -466 320 -664 216 -255 493 -416 846 -492 141 -31 424 -33 574 -5 525 97 943 433 1162 936 20 47 38 86 39 88 8 9 -9 -134 -26 -220 -46 -232 -175 -504 -334 -703 -293 -368 -677 -600 -1135 -688 -98 -19 -149 -22 -345 -22 -198 0 -247 3 -350 23 -312 60 -642 220 -877 427 -245 214 -429 491 -528 795 -49 147 -67 246 -79 434 -23 333 35 639 173 922 186 381 482 675 861 855 295 140 650 202 965 168z m764 -434 c268 -48 487 -284 515 -557 33 -333 -163 -622 -479 -705 -87 -23 -253 -22 -340 1 -168 45 -330 178 -407 332 -126 253 -79 552 116 744 162 160 371 225 595 185z"
            />
          </g>
        </svg>
      </button>
    </PRismTooltip>

    <PRismTooltip text="Open on GitHub" :as-child="true">
      <button
        type="button"
        class="pr-ext-links__btn"
        aria-label="Open on GitHub"
        @click="openUrl(url)"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="currentColor"
          aria-hidden="true"
        >
          <path
            fill-rule="evenodd"
            d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8Z"
          />
        </svg>
      </button>
    </PRismTooltip>
  </div>
</template>

<style scoped>
.pr-ext-links {
  display: inline-flex;
  align-items: center;
  gap: var(--s-1);
  flex-shrink: 0;
}

.pr-ext-links__btn {
  color: var(--text-mute);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: var(--r-2);
  background: transparent;
  border: 0;
  padding: 0;
  cursor: pointer;
  transition:
    color 0.12s,
    background 0.12s;
}

.pr-ext-links__btn:hover {
  background: var(--bg-3);
  color: var(--text);
}

.pr-ext-links__btn:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
  color: var(--text);
}
</style>
