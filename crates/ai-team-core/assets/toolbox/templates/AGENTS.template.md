# <project> — project knowledge

<One line: what this is and who it's for.>

**Stack:** <languages / frameworks / notable deps>

**Layout**
- `<path>` — <what lives here, one line>
- `<path>` — <…>

**Commands** (from repo root)
- Install: `<cmd>`
- Lint: `<cmd>`
- Test: `<cmd>`
- Build / codegen: `<cmd>`
- Deploy: `<cmd>`

## Design (optional — greenfield / design-driven projects)

<Delete if not design-driven. Otherwise make the guide binding:>

The design guide in `design/` is the source of truth. Build against it; when a
decision isn't covered, ask and record it there. Key docs: `design/concept.md`
(vision + pillars), `design/design-system.md` (brand/tokens).

## Hard rules

<Only the non-obvious, safety-critical, or costly-to-get-wrong facts an AI
would otherwise get wrong. Skip anything the model can read from the code. For
each: state the rule, why it matters, the exact snippet/command, and how it's
enforced.>

### 1. <Rule name>
<Why it matters.>

```<lang>
<exact snippet or command>
```

<How it's enforced — CI test, lint gate, review.>

### 2. <Rule name>
<…>
