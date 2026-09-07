# Osvell Film Library — Design Contract

## Visual direction

Letterboxd structure inside an Apple TV cinematic shell: full-bleed hero, glass pill navigation, poster-led shelves.

## Material system (three surfaces only)

| Surface | Use |
|---------|-----|
| Canvas | Near-black page and hero scrims |
| Solid | Settings, friend manager, stats, readable forms |
| Glass | Floating pill nav, filters, command palette, job banner |

## Shell

Immersive Home and film detail are full-bleed. Other pages use `--gutter` (48px, 24px under 1100px). 960px minimum: no horizontal page scroll.

## Ratings

- Filled stars: accent color per star.
- Empty stars: muted outline, not container accent.
- Compact mode always shows numeric score.

## Film detail sections (in order)

1. Film identity  
2. Your history  
3. Friends  
4. About the film  
5. TMDB community (labeled)  
6. Cast & crew  
7. Similar films  

## Accessibility

Reduced motion and reduced transparency preserve hierarchy with solid fallbacks.
