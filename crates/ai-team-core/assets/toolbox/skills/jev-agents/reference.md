# Jev API contract

Exact shapes, so nothing gets invented. Verified against docs.typesafe.ai on
2026-09-23. When in doubt re-read the source page: `/api.md`, `/primitives.md`,
`/sdk/python/api/types/questions.md`.

## The three question types

Every question has an id **you choose** (never sent to the model, so put the full
meaning in `instructions`), a `type`, `instructions`, and usually `criteria`.

| Type | `criteria` | Answer fields |
|---|---|---|
| `choice` | map of option -> description (or `null`), max 255 | `choice`, `probabilities`, `confidence` |
| `score` | **ordered array** of level descriptions, 2 to 10 | `score`, `legend`, `probabilities`, `confidence` |
| `noul` | optional `{true, false}` descriptions | `noul` only - no `confidence` |

`instructions` and every criteria value may be a string, object, or array. Use an
object when contrast helps: `{"question": ..., "focus": ..., "compare": [...]}`,
`{"what": ..., "not_for": ..., "examples": [...]}`. Those field names are yours,
not API keywords. Reference nested state by backticked path: `` `ticket.message` ``.

- `score` is a probability-weighted position along the levels and **can fall
  between two of them**. Normalize by `len(criteria) - 1` before combining scores
  from scales of different lengths.
- `confidence` is derived from how peaked `probabilities` is. If you only want the
  best option, read `choice`; reach for `confidence` to decide whether to act.

## HTTP

```http
POST https://api.typesafe.ai/v1/systemone
Authorization: Bearer $TYPESAFE_API_KEY
Content-Type: application/json

{
  "state": {"message": "I was charged twice for order A-104."},
  "model": "jev-latest",
  "questions": {
    "department": {
      "type": "choice",
      "instructions": "Which team should handle `message`?",
      "criteria": {"billing": "Charges, invoices, refunds", "orders": "Delivery or returns"}
    },
    "severity": {
      "type": "score",
      "instructions": "How severe is the problem?",
      "criteria": ["Cosmetic", "Degraded but a workaround exists", "Blocked entirely"]
    },
    "refund_requested": {"type": "noul", "instructions": "The customer asks for money back"}
  }
}
```

```json
{
  "model": "jev-1.13.0",
  "usage": {"input_tokens": 214, "output_tokens": 0},
  "answers": {
    "department": {"type": "choice", "choice": "billing",
                   "probabilities": {"billing": 0.93, "orders": 0.07}, "confidence": 0.91},
    "severity":   {"type": "score", "score": 1.43,
                   "legend": {"0": "Cosmetic", "1": "Degraded but a workaround exists", "2": "Blocked entirely"},
                   "probabilities": {"0": 0.0, "1": 0.57, "2": 0.43}, "confidence": 0.35},
    "refund_requested": {"type": "noul", "noul": 0.72}
  }
}
```

`GET /v1/models` lists the names the account may send.

Errors: `401` bad key · `422` malformed request (body names the field) · `429`
rate limited · `529` overloaded. Retry `429`/`529` with exponential backoff; the
SDKs already do and honour `retry-after`.

## Python

```python
from typesafe_sdk import Choice, Noul, NoulCriteria, Score, TypeSafeClient

with TypeSafeClient() as client:                    # key from TYPESAFE_API_KEY
    result = client.system_one(
        state={"message": "..."},                   # str | dict | list, never None
        questions={
            "department": Choice(
                instructions="Which team should handle `message`?",
                criteria={"billing": "Charges, invoices, refunds", "orders": None},
            ),
            "severity": Score(                      # criteria, NOT labels=
                instructions="How severe is the problem?",
                criteria=["Cosmetic", "Workaround exists", "Blocked entirely"],
            ),
            "refund_requested": Noul(
                instructions="The customer asks for money back",
                criteria=NoulCriteria(true="Asks for a refund or credit",
                                      false="A billing complaint with no remedy requested"),
            ),
        },
        model="jev-latest",                         # optional; this is the default
    )

result.choices["department"].choice          # str, plus .probabilities / .confidence
result.scores["severity"].score              # float, plus .legend / .probabilities / .confidence
result.nouls["refund_requested"].noul        # float 0-1
result.answers["department"]                 # every answer, untyped by kind
result.model, result.usage.input_tokens, result.request_id
```

`.choices` / `.scores` / `.nouls` are typed views over the same `.answers` dict.

Constructor: `TypeSafeClient(api_key=, model=, retry=RetryPolicy(...), timeout=,
headers=, base_url=)`. Env: `TYPESAFE_API_KEY`, `TYPESAFE_DEFAULT_MODEL`,
`TYPESAFE_BASE_URL`, `TYPESAFE_LOG_LEVEL`. `AsyncTypeSafeClient` mirrors it with
`await client.system_one(...)`. Questions may also be plain dicts
(`{"type": "noul", "instructions": ...}`), which is what dynamic question sets want.

Exceptions all derive from `TypeSafeError`. `TypeSafeAPIError` carries `.status`,
`.body`, `.headers`, `.request_id`; subclasses cover 400/401/403/404/422/429/5xx,
and `TypeSafeRateLimitError` adds `.retry_after_ms`.

## JavaScript / TypeScript

```ts
import { choice, noul, score, TypeSafeClient } from "@typesafe-ai/sdk";

const client = new TypeSafeClient();                      // TYPESAFE_API_KEY
const response = await client.systemOne({
  state: { message: "I was charged twice." },
  questions: {
    department: choice("Which team should handle `message`?", { billing: null, orders: null }),
    severity: score("How severe is it?", ["Cosmetic", "Workaround exists", "Blocked"]),
    refundRequested: noul("The customer asks for money back"),
  },
});

response.answers.department.choice;          // answer types are inferred from the questions
response.answers.severity.score;
response.answers.refundRequested.noul;
```

Signatures: `choice(instructions, criteria)`, `score(instructions, criteria)`,
`noul(instructions?, criteria?)`. Keep the key server-side in a web app.
