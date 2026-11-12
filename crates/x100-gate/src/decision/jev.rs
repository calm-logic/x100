//! TypeSafe AI's Jev, `POST /v1/systemone`. Pinned to `jev-1.14.0`: `jev-latest` moves.
//!
//! The input is `state`. Questions are a map from name to question, and a choice's `criteria` a
//! map from option to description; the question's name is not shown to the model, the options'
//! names and descriptions are. Answers come back under the questions' names. Input tokens are
//! `usage.input_tokens`. Errors are `{"detail": ...}`: a string for 400, a list of what failed
//! validation for 422. 429 and 529 are to be retried after a short wait.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use x100_core::decision::DecisionProvider;
use x100_core::money::Price;

use super::{ChoiceAnswer, DecisionModel, QUESTIONS, Reply, failed, json, labels};
use crate::classify::Question;

const NAME: &str = "Jev";

pub struct Jev;

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    answers: HashMap<String, ChoiceAnswer>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Failure {
    detail: Value,
}

impl DecisionModel for Jev {
    fn provider(&self) -> DecisionProvider {
        DecisionProvider::Jev
    }

    fn model(&self) -> &'static str {
        "jev-1.14.0"
    }

    fn path(&self) -> &'static str {
        "/v1/systemone"
    }

    fn price(&self) -> Price {
        super::input_price(42_000)
    }

    fn key_header(&self, key: &str) -> (&'static str, String) {
        ("authorization", format!("Bearer {key}"))
    }

    fn body(&self, question: &Question) -> Value {
        let questions: Map<String, Value> = QUESTIONS
            .iter()
            .map(|q| {
                let criteria: Map<String, Value> = q
                    .options
                    .iter()
                    .map(|(o, d)| ((*o).to_owned(), Value::from(*d)))
                    .collect();
                let asked = json!({
                    "type": "choice",
                    "instructions": q.instructions,
                    "criteria": criteria,
                });
                (q.key.to_owned(), asked)
            })
            .collect();
        json!({
            "model": self.model(),
            "state": super::input(question),
            "questions": questions,
        })
    }

    fn read(&self, status: u16, body: &[u8]) -> Reply {
        if status != 200 {
            let said = serde_json::from_slice::<Failure>(body)
                .ok()
                .map(|f| detail(&f.detail));
            return Reply {
                answer: Err(failed(NAME, status, said)),
                input_tokens: None,
            };
        }
        let answer: Answer = match json(NAME, body) {
            Ok(a) => a,
            Err(e) => {
                return Reply {
                    answer: Err(e),
                    input_tokens: None,
                };
            }
        };
        let input_tokens = answer.usage.and_then(|u| u.input_tokens);
        let mut answers = answer.answers;
        Reply {
            answer: labels(NAME, answers.remove("kind"), answers.remove("risk")),
            input_tokens,
        }
    }
}

/// A 400's text, or the first thing a 422 says failed.
fn detail(detail: &Value) -> String {
    match detail {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| {
                let msg = i.get("msg").and_then(Value::as_str)?;
                let at = i
                    .get("loc")
                    .and_then(Value::as_array)
                    .map(|l| {
                        l.iter()
                            .map(|p| p.as_str().map_or_else(|| p.to_string(), str::to_owned))
                            .collect::<Vec<_>>()
                            .join(".")
                    })
                    .unwrap_or_default();
                Some(if at.is_empty() {
                    msg.to_owned()
                } else {
                    format!("{at}: {msg}")
                })
            })
            .collect::<Vec<_>>()
            .join("; "),
        other => other.to_string(),
    }
}
