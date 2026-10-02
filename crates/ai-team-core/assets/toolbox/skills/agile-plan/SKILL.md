---
name: agile-plan
description: Plan a feature, ticket or change the way a product and engineering team would - an outcome grounded in the code, user stories that each ship as one PR with acceptance criteria and a demo, and engineering tasks with an owner - deciding what stacks and what builds in parallel. Use when asked to plan, break down or scope work before building it, or when a prompt says to plan it "agile".
---

# agile-plan - plan like a product and engineering team

A plan is a feature broken into **stories that each ship on their own**. Every story is
one PR; every PR is something a person can see working. Write it in this order.

## 1. Outcome
One paragraph: who gets what, and how anyone will know it worked. Say what is out of scope.

## 2. Grounding
Read before you plan: the code you will change, the patterns around it, its tests, the
ticket and any designs. Each fact names where it was verified. An assumption you did not
check is an open question, not grounding.

## 3. Decisions
Numbered, each with its reason, so a later story cannot quietly re-open it.

## 4. Stories - one per PR
- **Story:** As a <who>, I want <what>, so that <why>.
- **Acceptance criteria:** Given / When / Then, each one testable.
- **Demo:** the exact steps that show it working.
- **Size:** reviewable in one sitting. Split by behaviour (a thinner working slice), never
  by layer - "all the backend, then all the UI" ships nothing until the end.
- **Base:** the default branch, unless the story needs code from an earlier one - then
  stack it on that story's branch. Stack only for a real code dependency: siblings build
  and review in parallel, and every fix to a stack's root ripples through the rest.
  Nothing is built on the default branch itself.

## 5. Tasks - inside a story
The engineering steps in build order, one owner each, naming the paths they touch:

```
## Tasks
- T1 [backend] Add the range column and its migration - Touches: migrations/**, src/db/**
- T2 [frontend] Show the range picker on Summary - Touches: ui/src/Summary.tsx
```

Order them so the tree builds after each task where it can: data, then API, then UI,
then docs. The owner is the role that knows those paths; a task two roles need is two tasks.

## 6. Definition of done - every story
The project's own checks pass (typecheck, lint, test, build); every acceptance criterion
is demonstrated; the change is reviewed; docs and rules it makes stale are updated in the
same PR. No sprints and no story points - size by reviewability, not by calendar.

## 7. Open questions
What only a human can decide. Ask rather than guess, and say which story it blocks.

## Writing it down
If `aip` is on PATH the plan lives in ai-planner, not a file: `aip new` (with `--base`),
`aip section` for outcome and grounding, `aip decision add`, `aip question add`, and one
`aip slice add <PRn> "<title>" --base <branch> --branch <branch> --demo "…" --scope-file
<file>` per story, whose scope holds the story, its acceptance criteria and its `## Tasks`.
Otherwise present the plan in the conversation and let the user say where it goes.
