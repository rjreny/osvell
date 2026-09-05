# Stats & Settings Redesign

**Date:** 2026-09-05  
**Status:** Locked  
**Surfaces:** `StatsView`, `SettingsView` (+ related styles in `materials.css`)  
**Ship order:** Settings structural replacement → Stats hierarchy/deletion pass → polish / responsive / accessibility

---

## Design principle

These are utility surfaces within a film product, not dashboard surfaces. Structure comes from typography, spacing, imagery, and local dividers—not card chrome.

**Litmus test for review:** if a change would look at home in a generic SaaS analytics or admin settings template after swapping the movie data for any other dataset, reject it.

Aligns with `PRODUCT.md` (calm, Apple TV–level restraint; avoid generic AI-dashboard aesthetics) and `DESIGN.md` (Solid surface for Stats/Settings; poster-led product language elsewhere).

---

## Goals

1. Make **Stats** feel like a personal film retrospective, not an analytics dashboard populated with movie fields.
2. Make **Settings** feel like a focused preference shell for a desktop film app, not a full-bleed developer document or OS settings clone.
3. Preserve Studio’s unboxed Solid surface: no KPI tiles, no wrapping every control in a card.
4. Keep the existing shell (pill nav, search) unchanged; raise page content to that standard.

---

## Non-goals

- Redesigning Home, Films, Friends, Taste, or the global shell.
- Adding new Stats analytics (new chart types, filters beyond a quiet scope control, social comparisons).
- Expanding Settings into account/cloud/multi-device management.
- Replacing the material system (Canvas / Solid / Glass).

---

## Stats

### Intent

A retrospective of the user’s log: overview → cinema (posters) → one deliberate data centerpiece → supporting habits → textual rankings.

### Layout (top → bottom)

1. **Header**
   - Title: `Stats`
   - Quiet scope control: `All time` (visually secondary; not a filter toolbar)
2. **Overview (typography only)**
   - Four primary figures in a row, large type on the page background:
     - films · watches · hours watched · average rating
   - Secondary facts as small captions under the relevant primary (e.g. rewatches under watches; rated count under avg)—not a middot-joined strip
   - **Do not include watchlist** in this band (not a viewing statistic)
   - **Responsive:** at reduced widths, preserve hierarchy by wrapping to **2×2** before collapsing to a vertical list. Do not convert metrics into cards at any breakpoint.
3. **Most rewatched**
   - Poster shelf immediately under overview
   - Structurally important product signal; captions show watch counts (`5×`, etc.)
4. **Activity + Ratings (one row)**
   - **Watching activity:** dominant visualization (~65% of row width); the page’s visual data centerpiece; enough height to feel deliberate
   - **Rating habits:** supporting (~30–35%); distribution chart plus compact facts (avg / five-star / rated %)
5. **Genres | Decades (one row)**
   - **Genres:** ranked list with proportional bars, counts, and average ratings
   - **Decades:** compact ranked list (label + count)—typography and spacing only
6. **Highest rated** (optional)
   - Poster shelf below Genres/Decades if retained
   - Must not compete with Most rewatched (quieter heading, same shelf pattern, no equal visual weight)

### Anti-regression (Stats)

| Constraint | Detail |
|------------|--------|
| No KPI cards | Large numbers sit on the page; no rounded rectangles / dashboard tiles at any breakpoint |
| Overview wrap | 2×2 before vertical list; never convert metrics into cards |
| No Genre Affinity | Remove entirely; do not replace with another “advanced” viz |
| Posters early | Most rewatched stays near the top; not a decorative afterthought |
| Activity owns the row | ~65% activity / ~35% ratings; do not equalize |
| Ranked lists, not chart-lib | Genres/Decades use type, bars, counts, ratings—not histograms or scatter plots |
| Quiet scope | `All time` stays subdued; header is not a filter chrome strip |
| Highest rated optional | If present, subordinate to Most rewatched |
| No card chrome | Local hairlines and whitespace only; dividers belong to the content region |
| No clinical middot hero | Do not restore `483 films · 500 watches · …` as the primary overview |

### Data / behavior notes

- Prefer existing `getStats` / coverage / library data; redesign is primarily composition and CSS/JSX structure.
- Activity outlier handling (e.g. import spike) may be annotated or scaled in a later polish pass; do not invent a second competing chart to “fix” it.
- `All time` may be UI-only initially if filtering is not implemented; do not fake multiple scopes.

---

## Settings

### Intent

Stable section navigation + one focused editing panel. Utility density without ultrawide emptiness or LLM-playground aesthetics.

### Shell

- Content shell uses **`max-width` ~960px** (target class), with width otherwise constrained by available page space—not a fixed absolute width, and not edge-to-edge across the app viewport
- Inside that max: **Rail ~150–180px** + **panel ~650–750px**
- Rail items (fixed IA):

```
Library
Appearance
Taste
System
```

- **Only one section** is shown in the main panel at a time
- Section dividers and hairlines are local to the panel column—never stretched across the full application viewport
- No wrapping every setting in a card; use rows, whitespace, grouping, and local hairlines

### Library

- Letterboxd identity + connection status (e.g. username + Connected)
- Short status line (`Last synced …`)
- Actions with **state-dependent** weight (human label `Import history` preferred over `Import full export` unless terminology is required for accuracy):
  - **Before / import-needed** (no established library yet): `Import history` is primary; `Sync now` and `Match posters` are secondary
  - **After library is established** (history already imported / meaningful film count present): `Sync now` is primary or equal emphasis with Import; `Match posters` remains secondary
  - Do not permanently elevate Import as the sole primary once the library exists—it is occasional onboarding/recovery
- Long RSS / import explanation behind progressive disclosure: `How syncing works ›`
- TMDB: `Configured` / status + `Change`; Credential Manager detail is tertiary or revealed with Change—not bright permanent blue body copy

### Appearance

- **Theme:** restrained miniature surface previews for System / Dark / Light (~110×68 class size)—not oversized theme cards, not anonymous segment-only if previews fit cleanly
- **Accent:** Studio blue vs System as a quiet choice (dot / radio row), not a large control grid

### Taste

**Default panel (collapsed):**

- Recommendation model: selected name + one-line quality hint + `Change ›`
- Web research: short label + On/Off
- OpenRouter: key status (`Configured`) + `Change`

**Model selection:** centered **desktop modal** (dialog)—not an inline expanding four-column grid, and not a side sheet on desktop. A sheet is permitted only for narrow/mobile layouts. Metadata (context size, pricing class, long descriptions) lives inside the picker, muted.

Progressive disclosure is architectural: sync explanation and model metadata must leave the default view.

### System

Two **separate subsections** colocated under System (not merged conceptually):

1. **This PC** — app data size, database/health summary, storage path, `Open folder ›` (and related recovery actions as needed: log, reset, uninstall—kept secondary)
2. **Updates** — Studio version, up-to-date / available status, `Check for updates` (+ install when pending)

**Update available (emphasis without IA change):**

- Rail may show `System` with a quiet badge or `Update available`
- Updates subsection inside System becomes temporarily prominent
- Must **not** add a fifth rail item, footer status strip, or second navigation system

### Anti-regression (Settings)

| Constraint | Detail |
|------------|--------|
| Four-item rail fixed | Library / Appearance / Taste / System only |
| One panel section | No stacked full-page dump of all groups |
| Width locked | `max-width` ~960px shell; no viewport-spanning rules |
| No card farms | Rows + grouping + local hairlines |
| Progressive disclosure | Sync docs + model metadata off the default view |
| Model picker = desktop modal | Centered dialog on desktop; sheet only if narrow; never permanent multi-column model grid on the page |
| Theme previews restrained | Miniature surfaces, not big theme marketing cards |
| System subsections | This PC and Updates remain distinct blocks |
| Update badge ≠ new nav | Emphasis only; IA unchanged |

### Copy principles

- Prefer human labels (`Import history`, `Configured`, `Change`)
- Implementation detail (RSS cadence, Credential Manager, OpenRouter mechanics) is secondary or disclosed on demand
- One short supporting line per group max on the default view

---

## Visual system (shared)

- **Text levels:** primary (titles / large values), secondary (labels / status), tertiary (hints / paths / metadata)
- **Stronger size contrast** than today’s near-flat 12–14px field
- **Pills:** reserve for true segmented choices; do not pill every action
- **Dividers:** short, content-width, belonging to the region they separate
- **Materials:** Solid page surface per `DESIGN.md`; do not introduce Glass panels as section chrome
- **Motion:** product-register only (150–250ms state changes); no page-load choreography

---

## Accessibility

- Preserve WCAG 2.2 AA contrast; visible focus; keyboard access for rail, disclosure, and modal/sheet
- Theme previews must not rely on color alone (selected state + label)
- Model modal (and any narrow sheet fallback): focus trap, Escape to dismiss, restore focus to `Change`
- Respect `prefers-reduced-motion` and reduced-transparency fallbacks already in the app

---

## Implementation sketch (non-binding)

| Area | Likely touchpoints |
|------|--------------------|
| Settings shell | `SettingsView.tsx`, settings blocks in `materials.css` |
| Model picker | Extract/reuse `TasteModelList` inside a centered desktop modal; collapse default Taste UI |
| Stats composition | `StatsView.tsx`; remove `GenreAffinity`; reorder shelves; rebuild overview + Genres/Decades lists |
| Charts | Keep histogram for activity + ratings only; drop decade histogram usage |
| Scope control | Quiet header control; wire filtering only if backend/UI already supports it |

Exact component splits are left to the implementation plan.

---

## Ship order

1. **Settings structural replacement** — rail + panel shell (`max-width` ~960px), section IA, progressive disclosure, System subsections, centered model modal, restrained theme previews, state-dependent Library action weight
2. **Stats hierarchy / deletion pass** — remove Genre Affinity; overview type (2×2 wrap before vertical); promote Most rewatched; Activity/Ratings split; Genres/Decades lists; optional Highest rated demotion
3. **Polish / responsive / a11y** — Stats overview/row stacking; Settings rail collapse/select under the shell max; contrast and focus pass; modal focus trap

---

## Success criteria

- A screenshot of Stats reads as a film product first (posters + type), with one clear data centerpiece—not three equal charts.
- A screenshot of Settings shows a bounded two-pane preference UI; ultrawide no longer exposes empty full-width hairlines.
- Default Taste view does not look like an LLM model zoo.
- Genre Affinity is gone with no substitute scatter/bubble viz.
- No new KPI cards or settings cards introduced.
- Rail remains exactly four items; update availability only badges System.

---

## Open items (out of scope for this lock)

- Whether Highest rated ships in v1 of the Stats pass (optional; default lean: include quieter shelf if data exists)
- Exact activity outlier treatment (annotation vs scale)—polish pass
- Wiring `All time` to real filters—optional follow-up
- Precise responsive rail pattern (select vs collapse)—polish pass
