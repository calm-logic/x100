# Decision models

What x100 needs to know to call the three decision models the owner named for the gate's classifier. Read on
2026-11-12 from each vendor's own documentation. A decision model takes an input and a set of typed questions
and returns a typed answer with probabilities. It writes no text.

All three take an API key only. None runs on a subscription login.

| | Gemini Decision 1 | OpenAI Decisions | Jev (TypeSafe AI) |
|---|---|---|---|
| Call | `POST https://generativelanguage.googleapis.com/v1beta/decisions` | `POST https://api.openai.com/v1/decisions` | `POST https://api.typesafe.ai/v1/systemone` |
| Key | header `x-goog-api-key` | `Authorization: Bearer` | `Authorization: Bearer` |
| Model | `gemini-decision-1` | `gpt-6-luna`, the only one accepted | `jev-1.14.0`; `jev-latest` moves, so pin |
| Input | `input`: a string or content blocks | `input`: a string or messages | `state`: a string, object or array |
| Questions | a map of key to question, up to 1,000 | an array with optional `name`, up to 400 | a map of name to question |
| Yes or no | `noul`, optional `criteria` with `true_description`, `false_description` | `predicate`, instructions only | `noul`, optional `criteria` with `true`, `false` |
| Choice | `criteria`: a map of option to description, 2 to 255 | `choices`: an array of `{value, description}`, 2 to 255 | `criteria`: a map of option to description, 1 to 255 |
| Score | `criteria`: an array of level descriptions, lowest first | `levels`: an array of `{label, description}`, 2 to 10 | `criteria`: an array of level descriptions |
| Choice answer | `choice`, `probabilities` (map), `confidence` | `choice`, `probabilities` (array of `{value, probability}`), `confidence` | `choice`, `probabilities` (map), `confidence` |
| Yes or no answer | `noul` from 0.01 to 0.99, `confidence` | `probability` | `noul` from 0 to 1 |
| Price | $0.10 per million input tokens, output free, and a free tier | $0.10 per million input tokens, output free | $0.042 per million input tokens, output free |
| Context | 262,144 tokens | not stated in tokens | 64k a request, 32k a question, by default |
| Status | v1beta, Gemini Developer API only | generally available | keys from the console; third parties call it waitlisted |

Details that matter to a client:

- **Gemini.** The model never sees a question's key, so the whole question goes in `instructions`. Options are
  sorted by key before they are judged, and a tie goes to the first. A choice always spreads its probability
  over the options even when none fits, so give it a catch-all option. A yes or no never goes above 0.99.
  Errors are `{"error": {"code", "message"}}` with 400, and 429 `RESOURCE_EXHAUSTED`. Free tier: 200 requests a
  minute, 10,000 a day.
- **OpenAI.** One question may come back as `{"type": "refusal"}` while the others are answered. Error bodies
  are not documented. A third party saw 403 "Decision API is not enabled for this user" during the beta.
- **Jev.** The question's name is not shown to the model; option names and descriptions are. 400 is
  `{"detail": ...}`, 422 is `{"detail": [...]}`. 429 and 529 are retried with backoff. A paper (arXiv
  2609.26758) reports that it can follow an option's name over the description bound to it, so name options
  for what they mean.

Where each puts its answers and its token count, as the adapters read them: OpenAI's answers are an array in
question order with `name`, and usage is `usage.input_tokens`. Gemini's are keyed by question, usage is
`usage.total_input_tokens`, and a blocked input is `prompt_feedback.block_reason` with no answers. Jev's are
keyed by name and usage is `usage.input_tokens`. Gemini's request body is inferred from its reference pages;
no real call has confirmed any of the three.

Not stated by any of them: a latency guarantee, or how `confidence` is computed. Google claims 150 to 250 ms
end to end; that is the vendor's figure.

Vendors' advice x100 follows: set thresholds from labelled examples, treat the input as untrusted, and keep a
person on high-stakes actions.
