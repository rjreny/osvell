# Task 2 Report: Library panel

## Status

PASS — the Library panel now uses state-dependent action weight, progressive sync disclosure, and quieter TMDB configuration labels.

## Commit

- `0fc76c8` — `feat(settings): rewrite Library with disclosure and action weight`

## Changed files

- `studio/src/features/settings/SettingsView.tsx`
- `studio/src/features/settings/SettingsView.test.tsx`
- `studio/src/materials.css`

## TDD evidence

### RED

Added the Library behavior tests before changing production code, then ran:

```text
npm test -- src/features/settings/SettingsView.test.tsx
```

Result: exit 1, 6 failed and 1 passed. Every new assertion failed for the expected missing behavior:

- `Import history` did not exist.
- `Sync now` did not exist or become primary for any established-library signal.
- The once-an-hour sync explanation was visible by default.
- The configured TMDB row still exposed Credential Manager copy and old action labels.

```text
Test Files  1 failed (1)
Tests  6 failed | 1 passed (7)
```

### GREEN

After implementing the Library changes, reran the same focused command:

```text
npm test -- src/features/settings/SettingsView.test.tsx
```

Result: exit 0.

```text
Test Files  1 passed (1)
Tests  7 passed (7)
```

The final tests cover the empty-library primary action, all three established-library signals, secondary poster matching, collapsed sync help, and configured TMDB labels.

## Implementation

- Added `libraryEstablished(coverage)` with the required condition: meaningful movie count, full-history availability, or any source other than `none`.
- Relabeled the import action to `Import history` and the diary action to `Sync now`.
- Makes Import primary for an empty library and Sync primary after the library is established; Match posters remains secondary.
- Moved the hourly RSS/import explanation out of the default view behind an accessible `How syncing works` disclosure with `aria-expanded`.
- Retained a short, quiet last-refresh line in the default panel.
- Replaced the connected TMDB copy with `Configured` and the actions with `Change` / `Remove`.
- Removed permanent Credential Manager implementation detail from the default view.
- Scoped new action-weight and TMDB status styling to the Library panel so Appearance, Taste, and System remain unchanged.
- Added no cards and retained the Task 1 rail/shell.

## Verification

- Focused Settings test: PASS — 7/7.
- Production build (`npm run build`): PASS.
- Changed test lint (`npx eslint src/features/settings/SettingsView.test.tsx`): PASS.
- IDE diagnostics for all three changed files: no errors.
- `git diff --check`: PASS.

Repository-wide checks still expose unrelated existing failures:

- `npm test`: 26 passed, 3 failed, all in untouched `RecsView.test.tsx` because `New recommendation` has multiple matches. The same three tests fail when that file is run alone.
- `npm run lint`: 8 existing errors across `App.tsx`, `ConnectView.tsx`, `FriendsView.tsx`, `RecsView.tsx`, pre-existing import-boundary lines in `SettingsView.tsx`, and `platform/updater.ts`. No new lint diagnostics were introduced by this task.

## Self-review

- Scope is limited to the Library panel and its focused tests/styles.
- The established-library predicate matches the locked formula.
- Sync implementation detail is not mounted until the user opens the disclosure.
- Human labels are exactly `Import history`, `Configured`, and `Change`; the removal action is `Remove`.
- Only one Library action carries primary weight in either state, and Match posters is never primary.
- Appearance previews, Taste UI, and System layout were not changed.
- Unrelated dirty files were not staged or committed.

## Concerns

- Repository-wide test and lint baselines are not green for the unrelated pre-existing issues listed above.

## Review fixes

- Restored the shared `.settings-key-row .key-status { margin: 0; }` reset so Taste retains its established spacing.
- Kept the new muted color, 13px type, and error color scoped to `.settings-library-panel`.
- Added explicit coverage proving `uniqueMovies: 1` alone establishes the library while `source` remains `none` and `fullHistoryAvailable` remains `false`.

### Review-fix verification

```text
pnpm exec vitest run src/features/settings/SettingsView.test.tsx
```

Result: exit 0.

```text
Test Files  1 passed (1)
Tests  8 passed (8)
```

- IDE diagnostics for the changed CSS and test files: no errors.
- `git diff --check` for the changed CSS and test files: PASS.
