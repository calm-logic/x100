//! OpenAI's Decisions API, `POST /v1/decisions`, whose only model is `gpt-6-luna`.
//!
//! Questions are an array, each with an optional `name`; answers come back in the same order
//! with the same names. A choice's options are an array of `{value, description}`. Any one
//! question may be answered `{"type": "refusal"}` while the others are answered. Input tokens are
//! `usage.input_tokens`. Error bodies are not documented; OpenAI's usual `{"error": {"message"}}`
//! is read when it is there.

use serde::Deserialize;
use serde_json::{Value, json};
use x100_core::decision::DecisionProvider;
use x100_core::money::Price;

use super::{ChoiceAnswer, DecisionModel, QUESTIONS, Reply, Unanswered, failed, json, labels};
use crate::classify::Question;

const NAME: &str = "OpenAI";

pub struct OpenAi;

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    answers: Vec<Value>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Failure {
    error: Detail,
}

#[derive(Deserialize)]
struct Detail {
    message: Option<String>,
}

impl DecisionModel for OpenAi {
    fn provider(&self) -> DecisionProvider {
        DecisionProvider::OpenAi
    }

    fn model(&self) -> &'static str {
        "gpt-6-luna"
    }

    fn path(&self) -> &'static str {
        "/v1/decisions"
    }

    fn price(&self) -> Price {
        super::input_price(100_000)
    }

    fn key_header(&self, key: &str) -> (&'static str, String) {
        ("authorization", format!("Bearer {key}"))
    }

    fn body(&self, question: &Question) -> Value {
        let questions: Vec<Value> = QUESTIONS
            .iter()
            .map(|q| {
                let choices: Vec<Value> = q
                    .options
                    .iter()
                    .map(|(o, d)| json!({ "value": o, "description": d }))
                    .collect();
                json!({
                    "type": "choice",
                    "name": q.key,
                    "instructions": q.instructions,
                    "choices": choices,
                })
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
            let said = serde_json::from_slice::<Failure>(body)
                .ok()
                .and_then(|f| f.error.message)
                .or_else(|| {
                    let text = String::from_utf8_lossy(body).trim().to_owned();
                    (!text.is_empty()).then_some(text)
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
        let input_tokens = answer.usage.and_then(|u| u.input_tokens);
        Reply {
            answer: read_answers(answer.answers),
            input_tokens,
        }
    }
}

/// Finds each question's answer by its name. A refusal of either is no answer.
fn read_answers(answers: Vec<Value>) -> Result<super::Labels, Unanswered> {
    let mut found = [None, None];
    for answer in answers {
        let name = answer
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let Some(at) = QUESTIONS.iter().position(|q| q.key == name) else {
            continue;
        };
        if answer.get("type").and_then(Value::as_str) == Some("refusal") {
            return Err(Unanswered::new(format!(
                "{NAME} refused to answer the {name} question"
            )));
        }
        let parsed = serde_json::from_value::<ChoiceAnswer>(answer).map_err(|_| {
            Unanswered::new(format!(
                "{NAME}'s answer to the {name} question is not one it documents"
            ))
        })?;
        found[at] = Some(parsed);
    }
    let [kind, risk] = found;
    labels(NAME, kind, risk)
}
