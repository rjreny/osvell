# Osvell Film Library — Product Contract

## Core invariant

**A film is not a diary entry.**

Imports and RSS append or reconcile source events. Library state is derived from events plus explicit user state.

## Ownership

- `viewings` and `rating_events` are immutable source history.
- User actions create local source events with provenance.
- `user_movie_state` and `viewing_projections` are rebuildable; never delete history.
- `viewing_projections` determines effective watch counts. It collapses only proven cross-source duplicates and clearly malformed legacy adjacent pairs; explicit rewatches remain distinct.

## Source identity

Every event attaches to a `source_movie_record` before optional canonical `movies` linkage.

## Idempotency

Dedup by stable `source_record_key`, not movie title/year or ZIP hash.

## Coverage

RSS never implies full history. `LibraryCoverage.fullHistoryAvailable` is true only after a successful export import.

## Letterboxd constraints

- Official export and public RSS only.
- No scraping, no password collection.
