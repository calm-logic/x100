//! Gemini Decision 1, `POST /v1beta/decisions` on the Gemini Developer API.
//!
//! Questions are a map from key to question; answers come back under the same keys. A choice's
//! options are a map from option to description, judged in key order, a tie going to the first.
//! Input tokens are `usage.total_input_tokens`. Input that a safety filter blocks is answered with
//! `prompt_feedback.block_reason` and no answers. Errors are `{"error": {"code", "message"}}`.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use x100_core::decision::DecisionProvider;
use x100_core::money::Price;

use super::{ChoiceAnswer, DecisionModel, QUESTIONS, Reply, Unanswered, failed, json, labels};
use crate::classify::Question;

const NAME: &str = "Gemini";

pub struct Gemini;

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    answers: HashMap<String, ChoiceAnswer>,
    usage: Option<Usage>,
    prompt_feedback: Option<Feedback>,
}

#[derive(Deserialize)]
struct Usage {
    total_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Feedback {
    block_reason: Option<String>,
}

#[derive(Deserialize)]
struct Failure {
    error: Detail,
}

#[derive(Deserialize)]
struct Detail {
    message: Option<String>,
    status: Option<String>,
}

impl DecisionModel for Gemini {
    fn provider(&self) -> DecisionProvider {
        DecisionProvider::Gemini
    }

    fn model(&self) -> &'static str {
        "gemini-decision-1"
    }

    fn path(&self) -> &'static str {
        "/v1beta/decisions"
    }

    fn price(&self) -> Price {
        super::input_price(100_000)
    }

    fn key_header(&self, key: &str) -> (&'static str, String) {
        ("x-goog-api-key", key.to_owned())
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
            "input": super::input(question),
            "questions": questions,
        })
    }

    fn read(&self, status: u16, body: &[u8]) -> Reply {
        if status != 200 {
            let said = serde_json::from_slice::<Failure>(body).ok().map(|f| {
                match (f.error.status, f.error.message) {
                    (Some(s), Some(m)) => format!("{s}: {m}"),
                    (s, m) => s.or(m).unwrap_or_default(),
                }
            });
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
        let input_tokens = answer.usage.and_then(|u| u.total_input_tokens);
        if let Some(reason) = answer.prompt_feedback.and_then(|f| f.block_reason) {
            return Reply {
                answer: Err(Unanswered::new(format!(
                    "{NAME} blocked the input ({})",
                    super::short(&reason)
                ))),
                input_tokens,
            };
        }
        let mut answers = answer.answers;
        Reply {
            answer: labels(NAME, answers.remove("kind"), answers.remove("risk")),
            input_tokens,
        }
    }
}
