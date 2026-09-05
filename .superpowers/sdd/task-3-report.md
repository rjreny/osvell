# Task 3 Report: Appearance previews

## Status

PASS - Appearance now uses restrained theme previews and a quiet accent radio row.

## Changed files

- `studio/src/features/settings/SettingsView.tsx`
- `studio/src/features/settings/SettingsView.test.tsx`
- `studio/src/materials.css`

## TDD evidence

### RED

Added two focused tests before changing production code, then ran:

```text
npm test -- --run src/features/settings/SettingsView.test.tsx
```

Result: exit 1. Both new tests failed for the expected missing ARIA radio groups:

```text
Unable to find an accessible element with the role "radiogroup" and name "Theme"
Unable to find an accessible element with the role "radiogroup" and name "Accent"
Tests 2 failed | 8 passed (10)
```

### GREEN

After implementing the controls, reran the same command.

```text
Test Files 1 passed (1)
Tests 10 passed (10)
```

## Implementation

- Replaced the anonymous Theme segment with a labeled `radiogroup`.
- Added System, Dark, and Light radio buttons with controlled `aria-checked` state.
- Added fixed 110x68 miniature surface previews with distinct system, dark, and light treatments.
- Kept preview controls visually compact with no enclosing cards or dashboard-style grid.
- Replaced the Accent segment with a quiet `Studio blue` / `System` radio row.
- Preserved the existing `Theme`, `Accent`, `onTheme`, and `onAccent` interfaces.
- Did not change the Library, Taste, or System panels.

## Verification

- Focused Settings tests: PASS, 10/10.
- Production build (`npm run build`): PASS.
- IDE diagnostics for all three changed source files: no errors.
- `git diff --check`: PASS.

Repository-wide checks retain unrelated existing failures:

- Full test suite: 29 passed, 3 failed, all in untouched `RecsView.test.tsx` because `New recommendation` has multiple matches.
- Full lint: 8 existing errors in `App.tsx`, `ConnectView.tsx`, `FriendsView.tsx`, `RecsView.tsx`, the pre-existing import lines in `SettingsView.tsx`, and `platform/updater.ts`.

## Concerns

- The existing `app` accent token is outside this task's scope; the new row uses the brief's `Studio blue` product label while preserving the existing `onAccent("app")` value.
