# Stats & Settings Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Settings with a bounded four-section preference shell and recompose Stats as a film retrospective, per `studio/docs/superpowers/specs/2026-09-05-stats-settings-redesign-design.md`.

**Architecture:** Keep existing data loaders (`getStats`, `getCoverage`, `getLibrary`, key/taste/update APIs). Rebuild composition in `SettingsView.tsx` and `StatsView.tsx` with CSS in `materials.css`. Extract a centered model-picker modal that reuses `TasteModelList`. Prefer structural RTL tests over visual snapshots.

**Tech Stack:** React + TypeScript (Vite), Vitest + Testing Library, existing Studio Solid-surface CSS tokens in `materials.css` / `tokens.css`.

## Global Constraints

- Utility surfaces, not dashboards: typography, spacing, imagery, local dividers — not card chrome (spec litmus test).
- Settings rail fixed: Library / Appearance / Taste / System; one panel section at a time.
- Settings shell: `max-width` ~960px; no viewport-spanning section rules.
- Model picker: centered desktop modal; sheet only if narrow later; never inline 4-up grid on default Taste.
- Stats: delete Genre Affinity with no replacement; no KPI cards at any breakpoint; overview wraps 2×2 then vertical.
- Activity ~65% / Ratings ~35% of their row; Genres/Decades are ranked lists only.
- Ship order: Settings → Stats → polish/a11y.
- Do not redesign Home/Films/Friends/Taste nav shell.
- Prefer human copy (`Import history`, `Configured`, `Change`); hide implementation docs behind disclosure.

---

## File map

| File | Responsibility |
|------|----------------|
| `studio/src/features/settings/SettingsView.tsx` | Rail + panel shell; section bodies; Library/Appearance/Taste/System |
| `studio/src/features/settings/SettingsView.test.tsx` | Create: structural + interaction tests for Settings |
| `studio/src/features/settings/TasteModelModal.tsx` | Create: centered modal hosting `TasteModelList` |
| `studio/src/features/films/StatsView.tsx` | Retrospective layout; remove affinity |
| `studio/src/features/films/StatsView.test.tsx` | Create: structural tests for Stats |
| `studio/src/features/films/RecsView.tsx` | Keep exporting `TasteModelList` (consume from modal; no permanent Settings grid) |
| `studio/src/materials.css` | Settings shell/rail/panel; theme previews; Stats overview/lists/row splits; modal styles |
| Spec (reference only) | `studio/docs/superpowers/specs/2026-09-05-stats-settings-redesign-design.md` |

Optional small helpers (only if SettingsView grows unwieldy mid-pass):
- `studio/src/features/settings/libraryEstablished.ts` — pure predicate for action weight

---

### Task 1: Settings shell (rail + one panel)

**Files:**
- Modify: `studio/src/features/settings/SettingsView.tsx`
- Modify: `studio/src/materials.css` (settings blocks ~844–1000 and ~1895+)
- Create: `studio/src/features/settings/SettingsView.test.tsx`

**Interfaces:**
- Produces: `type SettingsSection = "library" | "appearance" | "taste" | "system"`; state `section: SettingsSection`; rail renders exactly those four labels; only the active section’s panel mounts/renders.

- [ ] **Step 1: Write failing shell tests**

```tsx
// SettingsView.test.tsx — mock filmLibrary / install / updater / listen like FilmDetailView.test.tsx
it("exposes exactly four rail destinations and shows one panel at a time", async () => {
  renderSettings({ coverage: emptyCoverage });
  expect(screen.getByRole("navigation", { name: /settings/i })).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: /^(Library|Appearance|Taste|System)$/ })).toHaveLength(4);
  expect(screen.getByRole("heading", { name: "Library", level: 2 })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
  expect(screen.getByRole("heading", { name: "Look", level: 2 })).toBeInTheDocument(); // or "Appearance" if renamed
  expect(screen.queryByRole("heading", { name: "Library", level: 2 })).not.toBeInTheDocument();
});
```

Use heading copy consistently: prefer panel title matching rail (`Appearance` not `Look`) unless product copy insists otherwise — **lock panel titles to Library / Appearance / Taste / System**.

- [ ] **Step 2: Run test — expect fail**

```bash
cd studio && pnpm exec vitest run src/features/settings/SettingsView.test.tsx
```

Expected: FAIL (no rail / still stacked groups).

- [ ] **Step 3: Implement shell**

In `SettingsView.tsx`:
- Add `const [section, setSection] = useState<SettingsSection>("library")`
- Structure:

```tsx
<div className="settings-page page-pad">
  <header className="page-head">...</header>
  <div className="settings-shell">
    <nav className="settings-rail" aria-label="Settings">
      {SECTIONS.map((id) => (
        <button
          key={id}
          type="button"
          className={section === id ? "is-on" : undefined}
          aria-current={section === id ? "page" : undefined}
          onClick={() => setSection(id)}
        >
          {LABELS[id]}
          {id === "system" && pendingVersion ? <span className="settings-rail-badge">Update available</span> : null}
        </button>
      ))}
    </nav>
    <div className="settings-panel">
      {section === "library" ? <LibraryPanel ... /> : null}
      {/* appearance / taste / system */}
    </div>
  </div>
  {/* existing UpdateOverlay stays */}
</div>
```

Inline panels as functions/components in the same file for this task if preferred; extract later only if needed.

CSS (illustrative):

```css
.settings-shell {
  display: grid;
  grid-template-columns: 160px minmax(0, 1fr);
  gap: 28px 32px;
  width: 100%;
  max-width: 960px;
}
.settings-rail {
  display: flex;
  flex-direction: column;
  gap: 2px;
  align-self: start;
}
.settings-rail button {
  /* quiet text buttons, not pills for every item */
}
.settings-panel {
  min-width: 0;
  max-width: 720px;
}
.settings-group {
  /* remove full-viewport border stretch; local hairlines only inside panel */
}
```

Move existing section markup into the matching panel branch without redesigning copy yet (Task 2–5 refine).

- [ ] **Step 4: Re-run tests — expect pass**

```bash
cd studio && pnpm exec vitest run src/features/settings/SettingsView.test.tsx
```

- [ ] **Step 5: Commit**

```bash
git add studio/src/features/settings/SettingsView.tsx studio/src/features/settings/SettingsView.test.tsx studio/src/materials.css
git commit -m "$(cat <<'EOF'
feat(settings): add bounded rail and single-panel shell

EOF
)"
```

---

### Task 2: Library panel (disclosure + state-dependent actions)

**Files:**
- Modify: `studio/src/features/settings/SettingsView.tsx`
- Modify: `studio/src/materials.css`
- Modify: `studio/src/features/settings/SettingsView.test.tsx`
- Optional create: `studio/src/features/settings/libraryEstablished.ts`

**Interfaces:**
- Consumes: `coverage: LibraryCoverage | null`
- Produces: `function libraryEstablished(coverage: LibraryCoverage | null): boolean`  
  Suggested: `Boolean(coverage && (coverage.uniqueMovies > 0 || coverage.fullHistoryAvailable || coverage.source !== "none"))`

- [ ] **Step 1: Failing tests**

```tsx
it("makes Import history primary when the library is empty", () => {
  renderSettings({ coverage: { ...empty, uniqueMovies: 0, source: "none", fullHistoryAvailable: false } });
  const importBtn = screen.getByRole("button", { name: /import history/i });
  expect(importBtn.className).toMatch(/primary/);
});

it("elevates Sync now after the library is established", () => {
  renderSettings({ coverage: { ...empty, uniqueMovies: 120, source: "export", fullHistoryAvailable: true } });
  expect(screen.getByRole("button", { name: /sync now/i }).className).toMatch(/primary/);
  expect(screen.getByRole("button", { name: /match posters/i }).className).not.toMatch(/primary/);
});

it("keeps sync explanation collapsed until disclosed", () => {
  renderSettings({ coverage: empty });
  expect(screen.queryByText(/once an hour/i)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /how syncing works/i }));
  expect(screen.getByText(/once an hour/i)).toBeInTheDocument();
});
```

- [ ] **Step 2: Run — expect fail**

- [ ] **Step 3: Implement Library panel**

- Relabel import button to `Import history` (keep same handler).
- `const established = libraryEstablished(coverage)`
- Primary class: `established ? sync : import`; both may share equal emphasis if you use dual primary carefully — prefer Sync primary when established, Import as `ghost`/secondary.
- TMDB row: status text `Configured` when connected; `Change` / `Remove` actions; Credential Manager only as tertiary under Change or muted one-liner — not bright permanent body link color.
- Disclosure: `<button type="button" className="text-btn">How syncing works</button>` toggling the existing long RSS paragraph.

- [ ] **Step 4: Tests pass**

- [ ] **Step 5: Commit**

```bash
git commit -m "$(cat <<'EOF'
feat(settings): rewrite Library with disclosure and action weight

EOF
)"
```

---

### Task 3: Appearance theme previews + accent row

**Files:**
- Modify: `SettingsView.tsx`, `materials.css`, `SettingsView.test.tsx`

**Interfaces:**
- Consumes: existing `theme: Theme`, `accent: Accent`, `onTheme`, `onAccent`

- [ ] **Step 1: Failing test**

```tsx
it("presents theme choices as labeled previews, not only anonymous segments", () => {
  renderSettings({});
  fireEvent.click(screen.getByRole("button", { name: "Appearance" }));
  expect(screen.getByRole("radio", { name: /system/i })).toBeInTheDocument();
  expect(screen.getByRole("radio", { name: /dark/i })).toBeInTheDocument();
  expect(screen.getByRole("radio", { name: /light/i })).toBeInTheDocument();
});
```

- [ ] **Step 2: Run — expect fail**

- [ ] **Step 3: Implement restrained previews**

```tsx
<div className="theme-preview-row" role="radiogroup" aria-label="Theme">
  {(["system", "dark", "light"] as const).map((t) => (
    <button
      key={t}
      type="button"
      role="radio"
      aria-checked={theme === t}
      className={`theme-preview is-${t}${theme === t ? " is-on" : ""}`}
      onClick={() => onTheme(t)}
    >
      <span className="theme-preview-surface" aria-hidden />
      <span>{label(t)}</span>
    </button>
  ))}
</div>
```

CSS: ~110×68 preview surface; miniature bg/surface/border suggestion; **not** large marketing cards. Accent: quiet radio/dot row `Studio blue` / `System`.

- [ ] **Step 4: Tests pass + commit**

```bash
git commit -m "$(cat <<'EOF'
feat(settings): add restrained theme previews in Appearance

EOF
)"
```

---

### Task 4: Taste collapsed default + centered model modal

**Files:**
- Create: `studio/src/features/settings/TasteModelModal.tsx`
- Modify: `SettingsView.tsx`, `materials.css`, `SettingsView.test.tsx`
- Reuse: `TasteModelList` from `RecsView.tsx`

**Interfaces:**
- Produces: `TasteModelModal({ open, models, selected, disabled, onClose, onPick })`
- Pattern: mirror `UpdateOverlay` / trailer overlay — `role="dialog"`, `aria-modal="true"`, Escape + backdrop click closes, restore focus to Change trigger.

- [ ] **Step 1: Failing tests**

```tsx
it("does not show the full model zoo on the default Taste panel", async () => {
  renderSettings({ tasteStatus: statusWithFourModels });
  fireEvent.click(screen.getByRole("button", { name: "Taste" }));
  expect(screen.queryByText(/1M ·/i)).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: /change/i })).toBeInTheDocument();
});

it("opens a centered dialog for model selection", async () => {
  renderSettings({ tasteStatus: statusWithFourModels });
  fireEvent.click(screen.getByRole("button", { name: "Taste" }));
  fireEvent.click(screen.getByRole("button", { name: /change/i }));
  const dialog = await screen.findByRole("dialog", { name: /recommendation model/i });
  expect(dialog).toBeInTheDocument();
  expect(within(dialog).getByText(/1M/i)).toBeInTheDocument();
});
```

- [ ] **Step 2: Run — expect fail**

- [ ] **Step 3: Implement**

Default Taste panel:
- Selected model label + one-line blurb from `models.find(m => m.id === selected)` + `Change ›`
- Web research On/Off (keep `seg` — legitimate segmented choice)
- OpenRouter key row like TMDB (`Configured` / `Change`)

`TasteModelModal.tsx`:
- Centered overlay (CSS `place-items: center`), solid dialog surface — **not** a side sheet
- Title `Choose recommendation model`
- Body: `<TasteModelList ... onPick={(id) => { void pick; onClose(); }} />`
- Actions: Cancel + optional Use model if you defer apply — simplest: picking applies immediately then closes (match current Settings behavior)

Focus: on open, focus dialog; on Escape/close, return focus to the Change button via ref.

- [ ] **Step 4: Tests pass + commit**

```bash
git commit -m "$(cat <<'EOF'
feat(settings): collapse Taste models into a centered modal

EOF
)"
```

---

### Task 5: System subsections (This PC + Updates)

**Files:**
- Modify: `SettingsView.tsx`, `materials.css`, `SettingsView.test.tsx`

**Interfaces:**
- Consumes: existing `installInfo`, `coverage`, `version`, `pendingVersion`, update handlers

- [ ] **Step 1: Failing tests**

```tsx
it("colocates This PC and Updates under System without extra rail items", () => {
  renderSettings({ installInfo: sampleInstall, pendingVersion: "0.9.0" });
  expect(screen.getAllByRole("button", { name: /^(Library|Appearance|Taste|System)$/ })).toHaveLength(4);
  fireEvent.click(screen.getByRole("button", { name: /System/ }));
  expect(screen.getByRole("heading", { name: "This PC", level: 3 })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Updates", level: 3 })).toBeInTheDocument();
  expect(screen.getByText(/update available/i)).toBeInTheDocument(); // rail badge or panel emphasis
});
```

- [ ] **Step 2: Implement**

System panel:
1. **This PC** — size (`formatBytes`), health/summary line, mono path, `Open folder ›`, secondary Log / Reset / Uninstall
2. Local hairline
3. **Updates** — version, status note, Check / Update primary when pending

Rail badge when `pendingVersion` set; do not add a fifth rail item.

- [ ] **Step 3: Tests pass + commit**

```bash
git commit -m "$(cat <<'EOF'
feat(settings): nest This PC and Updates under System

EOF
)"
```

---

### Task 6: Stats overview + delete affinity + promote posters

**Files:**
- Modify: `studio/src/features/films/StatsView.tsx`
- Modify: `studio/src/materials.css` (stats blocks ~1969+)
- Create: `studio/src/features/films/StatsView.test.tsx`

**Interfaces:**
- Remove `GenreAffinity` component entirely from the file
- Overview: four figures without watchlist; quiet `All time` in header (UI-only OK)

- [ ] **Step 1: Failing tests** (mock `getLibrary` / `getCoverage` / `getStats`)

```tsx
it("does not render Genre Affinity", async () => {
  renderStats();
  await screen.findByRole("heading", { name: "Stats" });
  expect(screen.queryByText(/genre affinity/i)).not.toBeInTheDocument();
});

it("places Most rewatched before Watching activity", async () => {
  renderStats();
  const rewatched = await screen.findByRole("heading", { name: /most rewatched/i });
  const activity = screen.getByRole("heading", { name: /watching activity/i });
  expect(rewatched.compareDocumentPosition(activity) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

it("does not use a middot summary strip as the hero", async () => {
  renderStats();
  await screen.findByRole("heading", { name: "Stats" });
  expect(document.querySelector(".stats-summary")).toBeNull();
});
```

- [ ] **Step 2: Implement**

- Delete `GenreAffinity`, `affinityTone`, and the affinity section
- Replace `.stats-summary` middot strip with `.stats-overview` grid of four metrics (large `<strong>` + label + optional caption)
- CSS:

```css
.stats-overview {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 16px 24px;
  /* no cards, no background tiles */
}
@media (max-width: 900px) {
  .stats-overview { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@media (max-width: 520px) {
  .stats-overview { grid-template-columns: 1fr; }
}
```

- Move Most rewatched shelf immediately under overview
- Quiet header: `<span className="stats-scope">All time</span>` (not a toolbar)

- [ ] **Step 3: Tests pass + commit**

```bash
git commit -m "$(cat <<'EOF'
feat(stats): film-first overview and remove genre affinity

EOF
)"
```

---

### Task 7: Activity/Ratings split + Genres/Decades lists

**Files:**
- Modify: `StatsView.tsx`, `materials.css`, `StatsView.test.tsx`

- [ ] **Step 1: Failing tests**

```tsx
it("keeps activity and ratings as a single asymmetric row", async () => {
  renderStats();
  await screen.findByRole("heading", { name: /watching activity/i });
  const row = document.querySelector(".stats-viz-row");
  expect(row).toBeTruthy();
  expect(row!.querySelector(".stats-activity")).toBeTruthy();
  expect(row!.querySelector(".stats-ratings")).toBeTruthy();
});

it("renders genres and decades as ranked lists without decade histogram", async () => {
  renderStats();
  await screen.findByRole("heading", { name: /genres/i });
  expect(document.querySelector(".stats-decades .stats-histogram")).toBeNull();
  expect(document.querySelector(".stats-rank-list")).toBeTruthy();
});
```

- [ ] **Step 2: Implement**

```tsx
<div className="stats-viz-row">
  <section className="stats-section stats-activity">...</section>
  <section className="stats-section stats-ratings">...</section>
</div>
<div className="stats-rank-row">
  <section className="stats-section stats-genres">ranked bars</section>
  <section className="stats-section stats-decades">ranked counts</section>
</div>
```

CSS:

```css
.stats-viz-row {
  display: grid;
  grid-template-columns: minmax(0, 1.85fr) minmax(0, 1fr); /* ~65/35 */
  gap: 24px 32px;
}
.stats-genre-bar {
  /* thin proportional fill; not chart-lib chrome */
}
```

- Highest rated: keep below ranks with quieter heading if data exists; do not match Most rewatched visual weight
- Remove old three-column `.stats-primary-row` layout

- [ ] **Step 3: Tests pass + commit**

```bash
git commit -m "$(cat <<'EOF'
feat(stats): asymmetric activity row and ranked genre/decade lists

EOF
)"
```

---

### Task 8: Polish, responsive, accessibility

**Files:**
- Modify: `materials.css`, `SettingsView.tsx`, `TasteModelModal.tsx`, `StatsView.tsx` as needed
- Extend tests for Escape-on-modal and rail keyboard if missing

- [ ] **Step 1: Checklist against success criteria (manual + tests)**

From the spec:
- [ ] Stats screenshot reads film-first (posters + type); one data centerpiece
- [ ] Settings bounded two-pane; no ultrawide full-width hairlines
- [ ] Default Taste is not a model zoo
- [ ] No Genre Affinity / no KPI cards / four rail items only
- [ ] Overview: 4 → 2×2 → 1 column, still unboxed
- [ ] Modal: focus trap or at least initial focus + Escape + restore focus
- [ ] Settings under ~720px: rail becomes select or horizontal compact list (pick one; prefer `<select>` or stacked rail above panel — document choice in commit message)

- [ ] **Step 2: Implement responsive rail**

```css
@media (max-width: 720px) {
  .settings-shell {
    grid-template-columns: 1fr;
  }
  .settings-rail {
    flex-direction: row;
    flex-wrap: wrap;
    gap: 4px;
  }
}
```

Avoid inventing a second footer nav.

- [ ] **Step 3: Run full relevant suite**

```bash
cd studio && pnpm exec vitest run src/features/settings/SettingsView.test.tsx src/features/films/StatsView.test.tsx
```

- [ ] **Step 4: Commit**

```bash
git commit -m "$(cat <<'EOF'
fix(ui): polish Stats/Settings responsive and a11y pass

EOF
)"
```

---

## Spec coverage self-check

| Spec requirement | Task |
|------------------|------|
| Design principle / no card chrome | Global + Tasks 1, 3, 6 |
| Settings max-width ~960px, local dividers | Task 1 |
| Four-item rail; one panel | Task 1 |
| Library disclosure + state-dependent primary | Task 2 |
| Theme previews restrained | Task 3 |
| Taste collapsed; desktop centered modal | Task 4 |
| System = This PC + Updates; update badges System | Task 5 |
| Stats overview type; 2×2 wrap; no watchlist middot strip | Task 6 |
| Delete Genre Affinity | Task 6 |
| Most rewatched early | Task 6 |
| Activity 65% / Ratings 35% | Task 7 |
| Genres/Decades ranked lists | Task 7 |
| Highest rated subordinate | Task 7 |
| Polish / a11y / narrow | Task 8 |
| Success criteria as QA checklist | Task 8 |

## Placeholder / consistency notes

- Panel heading for Appearance: use **Appearance** (not Look) to match rail.
- `libraryEstablished` predicate uses `uniqueMovies`, `fullHistoryAvailable`, or `source !== "none"`.
- Model metadata (`context · cost`) appears only inside the modal.
- Do not reopen Genre Affinity, rail IA, or card farms during polish.
