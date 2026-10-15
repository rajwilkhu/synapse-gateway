---
sidebar_position: 2
title: Jev lane
description: Ask TypeSafe System One (Jev) typed questions through the chat completions API, fall back to chat models when Jev is unavailable, and judge-then-extract candidates in one call.
---

Use the Jev lane when you need a decision rather than prose: which team should handle a
ticket, whether a message is urgent, how well a document matches a description. You send
typed questions in a `jev` block and TypeSafe System One (Jev) answers them against the
conversation with structured, scored answers. Use hybrid extraction when you want Jev to
judge a set of candidates and a chat model to extract data only from the ones that pass, in
one request.

## Set up a route

A route serves the Jev lane through its `typesafe` legs. Add chat legs after them if you
want the route to keep answering when Jev is unavailable:

```toml
[routes."ticket-triage"]
legs = [
  { provider = "typesafe", model = "jev-latest" },
  { provider = "vertex", model = "gemini-3.5-flash-lite", region = "us" },
]
```

`typesafe` legs need `TYPESAFE_API_KEY`; see [Providers](../configuration/providers.md#typesafe).
A route with `typesafe` legs only accepts requests that carry a `jev` block with questions,
and returns `400` to any other request.

## Ask typed questions

Add a `jev` block with a non-empty `questions` map to a chat completion request. Each entry
maps a question name to its definition: a `type` (`choice`, `score` or `noul`), the
`instructions`, and `criteria` where the type needs them. Synapse forwards the questions to
Jev verbatim; TypeSafe's documentation describes each type.

```json
{
  "model": "ticket-triage",
  "messages": [{ "role": "user", "content": "The invoice for March charged us twice." }],
  "jev": {
    "questions": {
      "department": {
        "type": "choice",
        "instructions": "Which team should handle this ticket?",
        "criteria": {
          "billing": "Invoices, payments, refunds",
          "technical": "Errors, outages, integrations"
        }
      },
      "is_urgent": { "type": "noul", "instructions": "Is this urgent?" }
    }
  }
}
```

Jev evaluates the questions against a **state**. By default the state is the request's
`messages`, serialised as a JSON string. Set `jev.state` to a string of your own to evaluate
something else, such as a document. The state and the questions share Jev's input budget, so
keep both focused.

Input guardrails apply to Jev lane requests as to any chat request; see
[Guardrails policy](../configuration/guardrails-policy.md).

## Read the answers

A successful response is a normal `chat.completion`:

- `choices[0].message.content` is a JSON string. Parse it to get Jev's `answers` map, keyed
  by question name. Each answer's shape depends on its question type.
- `model` is the Jev build that answered.
- `usage` carries Jev's input and output token counts.

## When Jev fails

Synapse tries the route's `typesafe` legs in order:

- A retryable failure, meaning a `429`, `408` or `5xx` response or a connection error, moves
  to the next `typesafe` leg.
- A non-retryable failure, such as malformed questions, stops the request with `400` and
  TypeSafe's error message. Chat legs cannot fix a bad question, so there is no fallback.

If every `typesafe` leg fails retryably, the route's remaining legs answer the request as a
**normal chat completion**: the content is the chat model's reply, not an `answers` map. If
you add chat legs, your client must handle both shapes; the response's `model` field tells
you which one you got. If the route has no other legs, the request fails with `502` and
error code `all_legs_failed`.

A request can carry a `vertex` block together with the `jev` block. It has no effect on
Jev, but if the request falls back and the `vertex` block has native features, the native
Vertex lane serves it from the route's `vertex` legs. See [Native Vertex features](native-vertex.md).

## Hybrid extraction

Hybrid extraction judges candidates with Jev, then extracts structured data only from the
candidates that pass. Add an `extract` spec to the `jev` block:

```json
{
  "model": "verify-orgs",
  "messages": [{ "role": "user", "content": "Find the official website of Acme Ltd." }],
  "jev": {
    "questions": {
      "c0_match": { "type": "noul", "instructions": "Candidate 0 is Acme Ltd's own website." },
      "c1_match": { "type": "noul", "instructions": "Candidate 1 is Acme Ltd's own website." }
    },
    "extract": {
      "floor": 0.7,
      "candidates": [
        { "key": "c0", "question": "c0_match", "text": "<page excerpt 0>" },
        { "key": "c1", "question": "c1_match", "text": "<page excerpt 1>" }
      ],
      "prompt": "Extract the organisation's contact details from candidate {{key}}:\n\n{{text}}",
      "response_schema": {
        "type": "object",
        "properties": { "email": { "type": "string" }, "phone": { "type": "string" } }
      }
    }
  }
}
```

| Field | Description |
|---|---|
| `candidates` | Each candidate has a `key`, the `text` to extract from, and the `question` that gates it, which must be a `noul` question in `questions`. |
| `floor` | Between 0 (exclusive) and 1. A candidate survives when its question's `noul` answer is at least the floor. |
| `prompt` | Extraction instructions. Must contain `{{text}}`, replaced by the candidate's text; `{{key}}` is replaced by its key. |
| `response_schema` | A JSON schema object the extraction must match. |

Synapse asks Jev the questions, then runs one extraction per surviving candidate on the
route's non-`typesafe` legs, with the prompt as a system message. The schema is sent as
`response_format` on the standard lane, or as `vertex.response_schema` on the native Vertex
lane when the request has native Vertex features. The route needs at least one chat leg.

The response adds a `jev` block beside the usual fields:

```json
{
  "object": "chat.completion",
  "model": "jev-latest",
  "jev": {
    "answers": { "c0_match": { "...": "..." }, "c1_match": { "...": "..." } },
    "survivors": ["c0"],
    "degraded": false
  },
  "choices": [{
    "index": 0,
    "message": { "role": "assistant", "content": "{\"c0\":{\"email\":\"...\",\"phone\":\"...\"}}" },
    "finish_reason": "stop"
  }],
  "usage": { "prompt_tokens": 812, "completion_tokens": 64, "total_tokens": 876 }
}
```

- `content` is a JSON string mapping each survivor's key to its extraction. It is absent
  when no candidate reaches the floor.
- `usage` adds up Jev's tokens and every extraction's.
- `degraded` is `true` when every `typesafe` leg failed retryably, in which case Synapse
  extracts from **all** candidates and `answers` is `{}`, or when a survivor's extraction
  failed, in which case that key is missing from `content`. The response keeps the same shape
  either way.

Synapse returns `400` when the request has `stream: true`, the route has no chat legs, the
request has native Vertex features but the route has no `vertex` leg, `floor` is out of
range, `prompt` lacks `{{text}}`, `response_schema` is not an object, or a candidate's
`question` is missing or not a `noul` question.

## Streaming

With `stream: true`, a Jev decision arrives as a single `chat.completion.chunk` holding the
whole JSON content, followed by the finishing chunk and `data: [DONE]`. If the request falls
back to chat legs, it streams like any chat completion. Hybrid extraction does not stream.

## Ledger and metrics

Each Jev answer writes a ledger row with provider `typesafe` and lane `jev`. Hybrid
extraction adds one row per successful extraction, all sharing the request's `request_id`.
`synapse_jev_extraction_total`, labelled by `route` and `degraded`, counts hybrid responses.
See [Cost ledger](cost-ledger.md).

## Jev passthrough

To call Jev without the chat completions format, send its native `{state, questions}` body
to `POST /typesafe/v1/systemone`. Synapse forwards the body verbatim with its own
`TYPESAFE_API_KEY`, sets `model` to `jev-latest` if you omit it, and returns TypeSafe's
status and body unchanged. There is no fallback and no streaming. Usage is recorded in the
ledger with lane `passthrough`, attributed with the same headers as chat requests. Without
`TYPESAFE_API_KEY`, the endpoint returns `400`. Requests are scanned under the `default`
policy, and no setting gives this endpoint a different policy.

## Jev lane and Jev router

The Jev lane answers your questions. The [Jev router](jev-router.md) is a different feature
that uses Jev to pick a model tier for ordinary chat requests. A `jev` block sent to a
`strategy = "jev"` route returns `400`.
