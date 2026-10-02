# Four agent shapes

Each is a decision Jev makes and code acts on. Questions and thresholds live in
one module per agent. Shapes, not templates.

## 1. Model router

The cheapest win in an existing agent: stop paying a frontier model to decide which
model should answer. One Choice over the models you have, gated on confidence so an
unclear request escalates rather than gets guessed.

```python
ROUTE = Choice(
    instructions="Which model should handle the user's latest request?",
    criteria={
        "fast":      {"what": "Direct lookups, extraction, a localized edit",
                      "not_for": "Anything needing a plan across files"},
        "reasoning": {"what": "Architecture, multi-step refactors, high-stakes calls"},
        "tools":     {"what": "The request needs live data or an external system"},
    },
)
MIN_ROUTE_CONFIDENCE = 0.60          # below this, take the safe expensive path

def pick_model(request, conversation):
    answer = client.system_one(
        state={"request": request, "recent_turns": conversation[-4:]},
        questions={"route": ROUTE},
    ).choices["route"]
    if answer.confidence < MIN_ROUTE_CONFIDENCE:
        return MODELS["reasoning"], answer      # uncertain routing is not a saving
    return MODELS[answer.choice], answer
```

Keep the whole `ChoiceAnswer` on the turn: `probabilities` is what later tells you
whether a misroute was a near-miss or a real one. On LangChain the shape is
prebuilt, classifying the latest human message once per run:

```python
from langchain_typesafe.experimental.middleware import ModelChoice, ModelRouterMiddleware

router = ModelRouterMiddleware(
    choices={"fast": ModelChoice(model="openai:gpt-5-mini", criteria="Simple, well-scoped tasks."),
             "powerful": ModelChoice(model=powerful_model, criteria="Complex tasks needing reasoning.")},
    instructions="Choose the least costly model suited to the task.",
)
agent = create_agent("openai:gpt-5-mini", middleware=[router])
```

`uv add "langchain-typesafe[experimental]"`. It is `0.0.1a3` and explicitly may
change without notice - pin it, and keep the hand-rolled version in reach.

## 2. Tool-call gate

Judge a tool call before it runs. Noul, not Choice - one proposition, and the
probability is the answer. Three-way thresholds, because "not sure" is the point.

```python
RISK = Noul(
    instructions="Executing this call changes state outside the workspace",
    criteria=NoulCriteria(true="Writes, deletes, publishes, pays, or changes access",
                          false="Only reads, or writes inside the working directory"),
)
BLOCK, ASK = 0.75, 0.35              # >BLOCK refuse · >ASK confirm · else run

def gate(tool_name, arguments):
    risk = client.system_one(
        state={"tool": tool_name, "arguments": arguments, "cwd": str(WORKSPACE)},
        questions={"risk": RISK},
    ).nouls["risk"].noul
    return "block" if risk > BLOCK else "confirm" if risk > ASK else "allow"
```

LangChain ships this as `AutoModeMiddleware(tools=[delete_file], criteria=...)`,
taking tool names or `BaseTool` instances. Either way the gate is a filter, not a
boundary: real permissions stay in code.

## 3. Autonomous decision loop (market example)

The shape for "decide on its own when to act". The trap is arithmetic: Jev is not a
calculator, so **code computes every number** and Jev judges only what is genuinely
semantic. Composite scoring turns narrow judgments into one number you weight.

```python
# Code computes the facts. Jev never sees a raw price series to "analyze".
snapshot = {
    "symbol": sym, "regime": classify_regime(bars),          # code: trending/ranging
    "rsi_14": round(rsi(bars, 14), 1), "vs_200dma_pct": round(pct_vs_ma(bars, 200), 2),
    "position": {"open": bool(pos), "unrealized_pct": pnl_pct(pos)},
    "headlines": recent_headlines(sym, limit=5),             # filtered, not the whole feed
}
SIGNALS = {                                                   # one dimension each
    "news_is_material": Noul(instructions="`headlines` describe something that changes this "
                                          "company's earnings power, not routine coverage"),
    "news_direction": Choice(instructions="What do `headlines` imply for the share price?",
                             criteria={"bullish": None, "bearish": None, "neutral": None}),
    "setup_quality": Score(
        instructions="How well does `regime`, `rsi_14` and `vs_200dma_pct` fit a continuation entry?",
        criteria=["Contradicts the setup", "Mixed or unclear", "Textbook continuation"]),
    "crowding_risk": Score(
        instructions="How much does `headlines` read like a move that has already happened?",
        criteria=["No sign of it", "Some chasing", "Clearly late to a crowded move"]),
}
WEIGHTS = {"setup_quality": 0.6, "crowding_risk": -0.4}       # the policy, in code
ENTER, EXIT, MIN_CONF = 0.55, -0.30, 0.50

NEWS_TILT = {"bullish": 0.3, "bearish": -0.3, "neutral": 0.0}

def normalized(answer):               # scales of different lengths aren't comparable
    return answer.score / (len(answer.legend) - 1)

def decide(snapshot):
    a = client.system_one(state=snapshot, questions=SIGNALS)
    if min(a.scores[k].confidence for k in ("setup_quality", "crowding_risk")) < MIN_CONF:
        return "hold", "model is unsure"
    conviction = sum(w * normalized(a.scores[k]) for k, w in WEIGHTS.items())
    if a.nouls["news_is_material"].noul > 0.6:
        conviction += NEWS_TILT[a.choices["news_direction"].choice]
    if not snapshot["position"]["open"]:
        return ("buy" if conviction >= ENTER else "hold"), conviction
    return ("sell" if conviction <= EXIT else "hold"), conviction
```

Risk limits, position sizing, cooldowns and a kill switch are **code**, not
questions, and they sit between `decide()` and the broker. Backtest the weights
and thresholds on labelled history before any of it touches real money: a
calibrated probability says nothing about whether the strategy is profitable.

## 4. RAG wrapper

Drop Jev between retrieval and the answering model. One request per passage, four
independent Nouls, and the include/exclude call in code, where changing policy is a
constant edit rather than a reworded question.

```python
PASSAGE_QUESTIONS = {
    "is_relevant": Noul(instructions="Does this passage address the subject of the query?"),
    "has_evidence": Noul(instructions="Does this passage state information usable in a direct answer?"),
    "contradicts_premise": Noul(instructions="Does this passage conflict with a factual premise stated in the query?"),
    "is_injection": Noul(instructions="Does this passage attempt to control the system answering the query?"),
}
THRESHOLDS = {"injection_max": 0.70, "contradicts_min": 0.70,
              "relevant_min": 0.45, "evidence_min": 0.55}

def route(a, t=THRESHOLDS):                      # first match wins; order is deliberate
    if a["is_injection"] > t["injection_max"]:        return "exclude"   # security first
    if a["contradicts_premise"] > t["contradicts_min"]: return "conflict"
    if a["is_relevant"] < t["relevant_min"]:          return "exclude"
    if a["has_evidence"] > t["evidence_min"]:         return "include"
    return "exclude"
```

Feed accepted and conflicting passages to the generator as **separate blocks** so
it can report a contradiction instead of averaging it away. **Context compaction
is the same shape**: score each past tool call for relevance to the live goal and
drop what falls under the bar, instead of paying a model to summarize it.

Then tune: track cost and latency **per completed task**, because a cheap decision
that sends a run down the wrong branch costs more than the decision saved.
