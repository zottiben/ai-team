<!-- design/design-system.md — the brand/token source of truth the CODE reads.
     If Claude Design produced an interactive HTML board, link it below and keep
     this markdown as the machine-readable truth. The token literals here become
     the seed of a later code conformance gate. -->

# <Project> — Design system

> Status: draft · Author: <you> · Last updated: <date>
> Interactive style board (if any): <link to the Claude Design HTML artifact>

## Voice & tone
<How it should feel, in words. Do / don't.>

## Colour tokens
<Semantic names first (what code references), then the raw palette.>
- `bg` <#…> · `surface` <#…> · `text` <#…> · `text-muted` <#…> · `accent` <#…> · `danger` <#…>

## Typography
- Families: <heading / body / mono>
- Scale: <e.g. 12 / 14 / 16 / 20 / 24 / 32>

## Spacing & radius
- Spacing: <e.g. 4 / 8 / 12 / 16 / 24 / 32>
- Radius: <sm / md / lg / full>

## Elevation & motion
- Shadows: <…> · Motion: <durations / easing; honour reduced-motion>

## Components
<Core components + their states: default / hover / active / disabled / loading / empty / error.>
- **<Button>** — <variants + states>

## Usage rules
- Source everything from the tokens above — no raw hex / px / font literals at call sites.
- <Project-specific rules; these graduate into a conformance check as code grows.>
