# 0014 — Comment markdown rendering via GitHub `bodyHTML` + Shiki client highlighting

- **Status:** Accepted
- **Date:** 2026-05-21
- **Issue:** [#138](https://github.com/cerinoligutom/PRism/issues/138)
- **Deciders:** @cerinoligutom

## Context

Conversation surfaces render comment bodies as plain text. Both `ThreadsList.vue` (expanded thread comments) and `ReviewsTab.vue` (review summary bodies) bind `{{ comment.body }}` / `{{ entry.bodyTrimmed }}` to a single `<p>` with `white-space: pre-wrap`. The result is unstyled paragraphs - no code blocks, no links, no mentions, no formatting at all. Reviewers comparing a thread in PRism against the same thread on github.com lose every cue that markdown carries.

The conversation contract already accepts that two body shapes ride together: `bodyText` is the stripped-text snippet the thread head card preview consumes, and `body` is the markdown source that future rich rendering would consume. The data is in flight; the decision is how to render it.

Three knobs need pinning:

1. **Source of the rendered HTML.** GitHub's GraphQL exposes a pre-rendered `bodyHTML` on `PullRequestReviewComment`, `IssueComment`, and `PullRequestReview`. Alternatively a client-side markdown library can render `body` on demand.
2. **Syntax highlighting for fenced code blocks.** `bodyHTML` ships GitHub's CSS-class-tagged code blocks but no token-level highlighting; a client lib paired with Shiki / Prism would.
3. **Sanitisation discipline.** Any HTML the renderer emits gets piped through DOMPurify before reaching the DOM, regardless of source.

## Decision drivers

- **Visual parity with github.com is the headline value.** Reviewers should look at a thread and recognise it. Mentions, issue refs, emoji shortcodes, task-list checkboxes, tables, and quoted blocks should all match by default.
- **GFM extensions are GitHub's responsibility.** Hand-rolling `@mention` and `#123` linkification on top of a generic markdown parser is a recurring maintenance burden - one we'd own in perpetuity if approach B wins.
- **Payload size grows but is bounded.** PRism lazy-hydrates comment bodies per ADR 0010 - the larger `bodyHTML` only ships for PRs whose drawer / route the user actually opens.
- **Code-block highlighting is non-negotiable.** Code review comments quote source frequently; syntax-coloured code is part of the legibility floor.
- **No new attack surface.** Every HTML render passes through DOMPurify before reaching `v-html`. Whether the HTML came from GitHub or from a client lib, the sanitisation step is the same.

## Considered options

1. **GitHub `bodyHTML` + DOMPurify + Shiki for code blocks (chosen).** Pull `bodyHTML` on the existing GraphQL queries, persist alongside `body`, sanitise + render via `v-html`, walk the resulting DOM for `<pre><code class="language-*">` blocks and re-highlight with Shiki's bundled grammars. The rest of the markdown (links, mentions, refs, emoji, tables, task lists) ships ready-rendered.
2. **Client-side markdown library (`marked` / `markdown-it`) with GFM plugins + Shiki.** Render `body` on demand in the browser. Compact payload (markdown is the source-of-truth shape), full control of styling, no GitHub server-render dependency. But `@mention` and `#123` linkification, emoji shortcodes, and task-list rendering all need explicit plugin support; each maintained separately from GitHub itself.
3. **Status quo.** Keep `bodyText` plain rendering and accept the parity gap. Cheap to keep; expensive to live with.

## Decision

We will go with **Option 1** - render `bodyHTML` directly, sanitised, with Shiki layered on for code-block highlighting.

The visual-parity guarantee is structural: GitHub renders the canonical version, we render exactly that output. Mentions, refs, emoji, task lists, tables - every GFM feature lands automatically. The DOMPurify pass is the same regardless of source, and the payload growth is contained by the existing lazy-hydration boundary. Shiki provides the missing syntax highlighting via dual-theme inline CSS variables so the rendered output respects the app's light/dark theme without re-highlighting on toggle.

## Consequences

### Positive

- Comment surfaces match github.com on first render. No per-feature linkification to maintain client-side.
- Mentions, issue references, emoji shortcodes, task-list checkboxes, and tables work without bespoke parsing or plugin churn.
- Code blocks get token-level highlighting against `github-light` / `github-dark`; lazy grammar loading keeps the initial bundle small.
- The `PRismMarkdown` primitive centralises the render path - one place for sanitisation policy, link interception, and Shiki bootstrapping. Future call sites (issue comments, PR descriptions, comment composer previews) plug in without re-deriving the wiring.
- Legacy rows degrade cleanly: the primitive falls back to the plain `body` (pre-wrap) when `body_html` is NULL. No backfill required; the next sync cycle populates the column.

### Negative

- Payload size grows by approximately 3-5x on each comment / review summary that ships HTML. Bounded to the lazy-hydrator scope, but real.
- DOMPurify is a new runtime dependency (~50 KB). Necessary regardless of source, but the cost lands here.
- Shiki adds ~80 KB upfront plus per-language grammars on demand. Lazy loading keeps the initial paint cheap; only the languages a user actually views in a session get pulled.
- Three new TEXT columns (`review_comments.body_html`, `issue_comments.body_html`, `reviews.body_html`). NULL on legacy rows; the next cycle backfills.
- The CSP `img-src` directive expands to cover `*.githubusercontent.com` and `github.com` so user-content images and emoji shortcode assets render. No new exfiltration vector - the webview was already loading avatars from the same CDN.

### Neutral / follow-ups

- Image proxying via GitHub's `camo.githubusercontent.com` lands through `*.githubusercontent.com`. No change to behaviour - the webview cache handles repeat fetches.
- The thread head-comment snippet (`ThreadsList.vue`'s collapsed preview) stays plain text. The 2-line `-webkit-line-clamp: 2` CSS treatment doesn't survive rich rendering, and the snippet's purpose is "is this thread interesting to expand", not "render the full comment".
- Issue comments now carry `body_html` in the DTO + DB but aren't rendered as cards on the conversation surface today (M3 contract still applies). The column is populated for the future PR that lands the issue-comments tab.
- PR description and comment composer rendering are out of scope for v1. PRism is read-only; description rendering can layer on through the same primitive when an "Open description" affordance lands.

## References

- ADR [0006](0006-graphql-first-rest-fallback.md) - GraphQL-first stance, which the `bodyHTML` selection extension follows.
- ADR [0010](0010-conversation-depth-storage.md) - lazy-hydrate-on-detail-open strategy that bounds the payload growth.
- ADR [0013](0013-user-avatars-cache.md) - similar "extend an existing query, persist a new column, surface through a primitive" shape used as a structural template.
- Contract: [`docs/contracts/conversation-depth.md`](../contracts/conversation-depth.md) - documents the three new `body_html` columns + the `PRismMarkdown` primitive inline.
- [DOMPurify](https://github.com/cure53/DOMPurify) - sanitiser.
- [Shiki](https://shiki.style/) - syntax highlighter; dual-theme rendering via `defaultColor: false`.
