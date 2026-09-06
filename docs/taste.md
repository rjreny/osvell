# How Studio builds a recommendation

Taste is a hybrid recommendation pipeline: structured preference scoring and semantic comparison, with AI critique and narration around a validated shortlist. This overview describes the released v0.13.0 architecture; the development direction below is explicitly separate.

## From history to a shortlist

1. **Interpret the viewer's history.** Rating signals combine an absolute positive/negative interpretation with the viewer's own rating distribution. Likes, rewatches, and recency contribute additional weight. A watchlist entry is an expression of interest, not evidence that the viewer enjoyed a film.
2. **Build a profile and retrieve candidates.** Affinities span film craft, cast, genres, and keywords. Candidates come from related-film results, filmographies, friends, the watchlist, and exploration. Candidate provenance records how a film entered the pool.
3. **Compare candidates.** Structured scores combine content affinity, related-film evidence, friend affinity, recent taste, watchlist interest, novelty, and negative evidence. Where embeddings are available, semantic comparison adds similarity to positively and negatively rated history. Missing embeddings use a neutral fallback rather than fabricated similarity.
4. **Critique and optionally research.** An AI critic identifies superficial connections and gaps in the shortlist. When web discovery is enabled, targeted results must be resolved to catalog films, enriched, and scored before they can join the pool.
5. **Validate and explain.** Code checks eligibility and assembles the recommendation workspace. A separate narration step describes taste and supporting connections; its output does not control final list membership or order. Grounding and fallback logic constrain unsupported prose.
6. **Use feedback.** Feedback records interest, rejection, and already-seen status, with reasons that distinguish a poor match from a temporary mood. Relevant feedback adjusts future analysis.

An embedding is a numerical representation of a film's descriptive metadata. Comparing embeddings can find similarities beyond an exact shared genre or credit. It does not mean Studio has watched or directly understood the film's video or audio.

## What is being developed

The working implementation is exploring a clearer separation between personal fit, evidence confidence, film quality, and presentation diversity. Current work includes semantic candidate retrieval, eligibility bands that can retain promising discoveries with limited evidence, and diversification among close matches.

These are development directions, not promises about the current installer. More candidates and more complex scoring are only useful when evaluation shows better results.

## How to judge whether it is better

The released code includes replay evaluation; the working iteration adds retrieval and ranking benchmarks and calibration checks. The questions are practical: can the system recover held-out films a viewer liked, rank them above less suitable candidates, and avoid repetitive recommendations without sacrificing fit?

There is no published evidence here of superiority over other services or of broadly validated recommendation accuracy. Automated policy tests establish behavior, while recommendation quality needs representative, leakage-controlled holdouts and feedback from multiple viewers. Model-written explanations are not themselves proof of recommendation quality.

## Explore the released implementation

These links are pinned to v0.13.0 so the explanation remains auditable while the algorithm changes.

| Responsibility | Source |
| --- | --- |
| History signals | [preference.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/preference.rs) |
| Retrieval | [retrieve.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/retrieve.rs) |
| Scoring | [score.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/score.rs) |
| Semantic comparison | [semantic.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/semantic.rs) |
| Critique and narration | [reason.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/reason.rs) |
| Validation and assembly | [validate.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/validate.rs), [workspace.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/workspace.rs) |
| Feedback and evaluation | [feedback.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/feedback.rs), [eval.rs](https://github.com/rjreny/studio/blob/v0.13.0/studio/src-tauri/src/taste/eval.rs) |

## Data and online services

Studio persists its library and cached analysis locally. TMDB provides catalog matching and metadata, and Letterboxd public RSS provides recent diary activity. Taste sends film-history evidence and descriptive metadata through OpenRouter for model and embedding requests. Optional web discovery also makes online research requests. Provider availability, privacy practices, and usage charges apply.
