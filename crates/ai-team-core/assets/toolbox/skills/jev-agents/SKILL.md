---
name: jev-agents
description: Build agents and harnesses on Jev, TypeSafe's System One model - model routers, tool-call gates, autonomous decision loops, RAG passage filters, instant context compaction. Jev returns typed decisions with calibrated probabilities instead of text, so code branches on it directly. Use when adding Jev/TypeSafe to a project, wrapping an existing agent or RAG system in a decision layer, routing work between models, gating risky tool calls, or replacing a prompt-and-parse-JSON step with a typed one.
---

# jev-agents - agents whose decisions aren't LLM calls

Jev is a **System One model**: send it a `state` plus typed *questions*, get typed
answers with calibrated probabilities. It does not generate text, write code, or
explain itself. There is no `model: "jev-latest"` switch that turns a coding agent
into a Jev agent - a Jev agent is ordinary code that calls Jev where it used to
call an LLM to decide something.

Every build starts the same way: **find the decisions, not the text.**

| The step... | belongs to |
|---|---|
| creates prose, code, or a summary | the LLM |
| picks one of a known list, rates a rubric, answers yes/no | **Jev** |
| is an exact rule, arithmetic, a lookup, or the action itself | **code** |

Never give Jev a judgment code can compute exactly, and never let it execute
anything. A Jev call returns a decision; code acts on it.

## Wire facts (checked against the live docs, 2026-09-23)

| | |
|---|---|
| Endpoint | `POST https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer $TYPESAFE_API_KEY` |
| Model | `jev-latest` (SDK default) -> `jev-1.13.0`. Pin the versioned id once thresholds are tuned; an alias moves under you |
| SDKs | `pip install typesafe-sdk` · `npm i @typesafe-ai/sdk` (Node 20+) |
| Price | $0.042 per Mtok **input; output tokens are free** |
| Limits | 64k tokens per request (state + all questions), 32k for state + the longest single question |
| LangChain | `langchain-typesafe` - real, but `0.0.1a3` and its middleware sits under `experimental`. Read [recipes.md](recipes.md) before depending on it |

Exact request/response shapes, SDK signatures and error types: **[reference.md](reference.md)**.
Don't guess a field name; a made-up `labels=` or `.answer` is the single most
common way this integration breaks.

TypeSafe's own skill is vendored alongside this one (`ai-toolbox skill
typesafe-ai`). It covers the API surface and points at the live cookbooks; this
one covers agent and harness architecture on top of it. Loading both is fine.

## One request, many questions

Questions in one request are evaluated **in parallel against the same state** and
cannot see each other's answers. Ask everything the turn might need, including
speculative branches, and let code ignore what doesn't apply - extra questions
barely move latency and cost only their own tokens. One 13-question briefing came
out 12.2x cheaper and 10.0x faster than 13 sequential calls with no change in the
answers; firing those 13 concurrently closes the time gap but not the 13x on
tokens, because the state gets re-sent every time.

Make a second request only when you genuinely cannot build it until the first
answer lands (it decides what to fetch, or what the next options are).

## Sharp edges - `jev-1.13` fails in specific, knowable ways

These waste real debugging time. Design around them up front.

| Trap | Instead |
|---|---|
| **Arithmetic, counting, comparing numbers** | Compute in code. Ask one question per item and sum the answers yourself |
| **Dates** - ordering, ranges, "which is first" | Extract parts as Choices over closed sets, assemble and compare in code |
| **Literal reading** - it answers what you wrote, not what you meant | Put the exact condition in `instructions`, boundary cases in `criteria` |
| **Indirection** - a property of a property, double negatives | Name the state path in backticks: `` `ticket.sender.email` `` |
| **A big state full of irrelevant fields** | Retrieve and filter first. Unrelated material costs accuracy |
| **Adversarial text in the state** | State is data, not trusted. Be explicit in criteria and test it |
| **Assuming invariants** | `noul` and a yes/no Choice are *not* comparable, and `P(x) + P(not x) != 1`. Never carry a threshold across question types |
| **Score levels that are just numbers, or measure two things** | Describe a concrete situation per level; split multi-dimension scores and weight them in code |

## Thresholds are the product

Confidence (Choice/Score only - a Noul's value *is* its certainty) is the second
decision axis: act automatically / proceed with care / don't act. Set the bar by
what being wrong costs, so a destructive action gets a higher one than a read.

Put **every question and every threshold in one module**. They are what a human
reviews, and it keeps re-routing free: ask once over a labelled set, cache the
answers, then sweep the thresholds in code with no further API calls. Prove the
numbers that way before shipping - a cookbook threshold is a starting point, never
a default. Typed output guarantees the interface, not the truth.

## Build it

Four worked shapes - model router, tool-call gate, autonomous decision loop, RAG
wrapper - with real code: **[recipes.md](recipes.md)**.

The live docs are the source of truth and move faster than this file. Fetch
[llms.txt](https://docs.typesafe.ai/llms.txt) for the index, append `.md` to any
page path for clean Markdown, and read the closest cookbook before inventing a
decomposition - it usually shows a better one.
