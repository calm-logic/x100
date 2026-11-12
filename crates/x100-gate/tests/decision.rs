//! Each decision model's adapter, without a network: the request it builds for a sample action,
//! and what it reads from a good answer, an unsure one, a refusal, each documented error and
//! garbage.

mod common;

use common::*;
use serde_json::{Value, json};
use x100_core::action::{ActionKind, AskMeAbout, Risk};
use x100_core::decision::DecisionProvider;
use x100_core::money::{Money, Usage};
use x100_gate::decision::{DecisionModel, Reply, model};
use x100_gate::{Labels, Question, Verdict, assess};

const KINDS: [&str; 7] = [
    "deletes_data",
    "deploys_to_production",
    "spends_money",
    "sends_to_people_outside",
    "changes_credentials",
    "force_pushes",
    "none_of_these",
];
const RISKS: [&str; 3] = ["low", "medium", "high"];

fn question() -> Question {
    let (state, f) = fern(AskMeAbout::HighRiskOnly);
    let action = egress(f.backend, "api.mailer.example", "POST");
    let Verdict::Ask(p) = assess(&state, &action, None) else {
        panic!("no rule covers this host")
    };
    p.question().clone()
}

fn read(m: &dyn DecisionModel, status: u16, body: &Value) -> Reply {
    m.read(status, body.to_string().as_bytes())
}

fn labels(kind: ActionKind, risk: Risk, confidence: f64) -> Labels {
    Labels {
        kind,
        risk,
        confidence,
    }
}

fn why(reply: &Reply) -> String {
    reply.answer.clone().unwrap_err().why
}

/// The keys of a choice's options, as the provider's shape holds them.
fn options(question: &Value) -> Vec<String> {
    match (question.get("criteria"), question.get("choices")) {
        (Some(Value::Object(map)), None) => map.keys().cloned().collect(),
        (None, Some(Value::Array(list))) => list
            .iter()
            .map(|c| {
                assert!(c["description"].as_str().is_some_and(|d| !d.is_empty()));
                c["value"].as_str().unwrap().to_owned()
            })
            .collect(),
        other => panic!("no options: {other:?}"),
    }
}

fn sorted<const N: usize>(names: [&str; N]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
    v.sort();
    v
}

/// Checks the two questions as one provider lays them out: every option named and described,
/// the whole question in the instructions, and the agent's text only in the input.
fn check_questions(questions: Vec<(&str, &Value)>, q: &Question) {
    assert_eq!(questions.len(), 2);
    for (key, asked) in questions {
        assert_eq!(asked["type"], "choice");
        let instructions = asked["instructions"].as_str().unwrap();
        assert!(instructions.contains("untrusted"), "{instructions}");
        assert!(!instructions.contains("api.mailer.example"));
        let mut got = options(asked);
        got.sort();
        match key {
            "kind" => {
                assert_eq!(got, sorted(KINDS));
                assert!(instructions.contains("none_of_these"));
            }
            "risk" => {
                assert_eq!(got, sorted(RISKS));
                assert!(instructions.contains("If you are unsure, choose high."));
            }
            other => panic!("a question {other}"),
        }
    }
    assert!(q.input.contains("api.mailer.example"));
}

#[test]
fn each_provider_is_pinned_to_its_model_host_path_and_key_header() {
    let table = [
        (
            DecisionProvider::Gemini,
            "gemini-decision-1",
            "generativelanguage.googleapis.com",
            "/v1beta/decisions",
            ("x-goog-api-key", "ref_x".to_owned()),
        ),
        (
            DecisionProvider::OpenAi,
            "gpt-6-luna",
            "api.openai.com",
            "/v1/decisions",
            ("authorization", "Bearer ref_x".to_owned()),
        ),
        (
            DecisionProvider::Jev,
            "jev-1.14.0",
            "api.typesafe.ai",
            "/v1/systemone",
            ("authorization", "Bearer ref_x".to_owned()),
        ),
    ];
    for (provider, id, host, path, header) in table {
        let m = model(provider);
        assert_eq!(m.provider(), provider);
        assert_eq!((m.model(), provider.host(), m.path()), (id, host, path));
        assert_eq!(m.key_header("ref_x"), header);
    }
}

#[test]
fn input_tokens_are_charged_at_each_providers_price_and_output_is_free() {
    let usage = Usage {
        input: 1_000_000,
        output: 5_000,
        ..Usage::default()
    };
    for (provider, micros) in [
        (DecisionProvider::Gemini, 100_000),
        (DecisionProvider::OpenAi, 100_000),
        (DecisionProvider::Jev, 42_000),
    ] {
        assert_eq!(
            model(provider).price().cost(&usage),
            Money::from_micros(micros)
        );
    }
    let few = Usage {
        input: 1234,
        ..Usage::default()
    };
    // Rounded up to the next millionth of a dollar, so a budget is never undercounted.
    assert_eq!(
        model(DecisionProvider::Jev).price().cost(&few),
        Money::from_micros(52)
    );
}

#[test]
fn gemini_request_and_answers() {
    let m = model(DecisionProvider::Gemini);
    let q = question();
    let body = m.body(&q);
    assert_eq!(body["model"], "gemini-decision-1");
    assert_eq!(body["input"], q.input.as_str());
    let qs = body["questions"].as_object().unwrap();
    check_questions(qs.iter().map(|(k, v)| (k.as_str(), v)).collect(), &q);

    let good = |kind: &str, risk: &str, confidence: f64| {
        json!({
            "answers": {
                "kind": {"type": "choice", "choice": kind, "probabilities": {kind: 0.9}, "confidence": 0.9},
                "risk": {"type": "choice", "choice": risk, "probabilities": {risk: confidence}, "confidence": confidence},
            },
            "usage": {"total_input_tokens": 412, "total_output_tokens": 0, "total_tokens": 412},
        })
    };
    let r = read(m, 200, &good("sends_to_people_outside", "medium", 0.91));
    assert_eq!(
        r.answer,
        Ok(labels(ActionKind::SendOutside, Risk::Medium, 0.91))
    );
    assert_eq!(r.input_tokens, Some(412));
    let r = read(m, 200, &good("none_of_these", "low", 0.4));
    assert_eq!(r.answer, Ok(labels(ActionKind::Other, Risk::Low, 0.4)));

    let blocked = json!({
        "answers": {},
        "prompt_feedback": {"block_reason": "safety", "block_reason_message": "unsafe"},
        "usage": {"total_input_tokens": 30},
    });
    let r = read(m, 200, &blocked);
    assert!(
        why(&r).contains("blocked the input (safety)"),
        "{}",
        why(&r)
    );
    assert_eq!(r.input_tokens, Some(30), "a blocked input is still charged");

    let bad = json!({"error": {"code": 400, "message": "questions.kind.criteria: too few", "status": "INVALID_ARGUMENT"}});
    let r = read(m, 400, &bad);
    assert_eq!(
        why(&r),
        "Gemini answered 400: INVALID_ARGUMENT: questions.kind.criteria: too few"
    );
    assert!(!r.answer.clone().unwrap_err().again);
    let busy = json!({"error": {"code": 429, "message": "Quota exceeded", "status": "RESOURCE_EXHAUSTED"}});
    let r = read(m, 429, &busy);
    assert!(why(&r).contains("RESOURCE_EXHAUSTED"));
    assert!(r.answer.unwrap_err().again, "a rate limit may pass");

    check_garbage(m);
}

#[test]
fn openai_request_and_answers() {
    let m = model(DecisionProvider::OpenAi);
    let q = question();
    let body = m.body(&q);
    assert_eq!(body["model"], "gpt-6-luna");
    assert_eq!(body["input"], q.input.as_str());
    let qs = body["questions"].as_array().unwrap();
    check_questions(
        qs.iter()
            .map(|v| (v["name"].as_str().unwrap(), v))
            .collect(),
        &q,
    );

    let choice = |name: &str, choice: &str, confidence: f64| {
        json!({"type": "choice", "name": name, "choice": choice, "confidence": confidence,
               "probabilities": [{"value": choice, "probability": confidence}]})
    };
    let good = |risk_confidence: f64| {
        json!({
            "model": "gpt-6-luna",
            "answers": [choice("kind", "deletes_data", 0.8), choice("risk", "high", risk_confidence)],
            "usage": {"input_tokens": 230, "output_tokens": 0, "total_tokens": 230,
                      "input_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 0}},
        })
    };
    let r = read(m, 200, &good(0.97));
    assert_eq!(
        r.answer,
        Ok(labels(ActionKind::DeleteData, Risk::High, 0.97))
    );
    assert_eq!(r.input_tokens, Some(230));
    let r = read(m, 200, &good(0.3));
    assert_eq!(r.answer.unwrap().confidence, 0.3);
    // In either order: answers are found by name.
    let swapped =
        json!({"answers": [choice("risk", "low", 0.9), choice("kind", "force_pushes", 0.9)]});
    assert_eq!(
        read(m, 200, &swapped).answer,
        Ok(labels(ActionKind::ForcePush, Risk::Low, 0.9))
    );

    let refused = json!({
        "answers": [choice("kind", "deletes_data", 0.8), {"type": "refusal", "name": "risk"}],
        "usage": {"input_tokens": 230},
    });
    let r = read(m, 200, &refused);
    assert_eq!(why(&r), "OpenAI refused to answer the risk question");
    assert_eq!(r.input_tokens, Some(230));

    let off = json!({"error": {"message": "Decision API is not enabled for this user", "type": "invalid_request_error"}});
    assert_eq!(
        why(&read(m, 403, &off)),
        "OpenAI answered 403: Decision API is not enabled for this user"
    );
    // Error bodies are not documented, so whatever came is said.
    let r = m.read(502, b"<html>Bad gateway</html>");
    assert_eq!(why(&r), "OpenAI answered 502: <html>Bad gateway</html>");
    assert!(r.answer.unwrap_err().again);

    check_garbage(m);
}

#[test]
fn jev_request_and_answers() {
    let m = model(DecisionProvider::Jev);
    let q = question();
    let body = m.body(&q);
    assert_eq!(body["model"], "jev-1.14.0");
    assert_eq!(body["state"], q.input.as_str());
    assert!(body.get("input").is_none());
    let qs = body["questions"].as_object().unwrap();
    check_questions(qs.iter().map(|(k, v)| (k.as_str(), v)).collect(), &q);

    let good = |risk: &str, confidence: f64| {
        json!({
            "model": "jev-1.14.0",
            "answers": {
                "kind": {"type": "choice", "choice": "spends_money", "confidence": 0.88, "probabilities": {"spends_money": 0.88}},
                "risk": {"type": "choice", "choice": risk, "confidence": confidence, "probabilities": {risk: confidence}},
            },
            "usage": {"input_tokens": 120, "output_tokens": 12},
        })
    };
    let r = read(m, 200, &good("high", 0.75));
    assert_eq!(r.answer, Ok(labels(ActionKind::Spend, Risk::High, 0.75)));
    assert_eq!(r.input_tokens, Some(120));
    assert_eq!(
        read(m, 200, &good("medium", 0.5))
            .answer
            .unwrap()
            .confidence,
        0.5
    );

    let bad = json!({"detail": "max_tokens_exceeded"});
    assert_eq!(
        why(&read(m, 400, &bad)),
        "Jev answered 400: max_tokens_exceeded"
    );
    let invalid = json!({"detail": [{"loc": ["body", "questions", "kind", "criteria"], "msg": "too many entries", "type": "value_error"}]});
    assert_eq!(
        why(&read(m, 422, &invalid)),
        "Jev answered 422: body.questions.kind.criteria: too many entries"
    );
    for status in [429, 529] {
        let r = read(m, status, &json!({"detail": "slow down"}));
        assert!(r.answer.unwrap_err().again, "{status} is retried");
    }
    let r = read(m, 401, &json!({"detail": "Invalid API key"}));
    assert!(!r.answer.unwrap_err().again);

    check_garbage(m);
}

/// What no provider documents is no answer, never a guess.
fn check_garbage(m: &dyn DecisionModel) {
    let kind = |choice: &str| json!({"type": "choice", "choice": choice, "confidence": 0.9});
    let shaped = |answers: Value| -> Value {
        match m.provider() {
            DecisionProvider::OpenAi => {
                let list: Vec<Value> = answers
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(name, a)| {
                        let mut a = a.clone();
                        a["name"] = Value::from(name.as_str());
                        a
                    })
                    .collect();
                json!({ "answers": list })
            }
            _ => json!({ "answers": answers }),
        }
    };
    let bodies: Vec<(&str, Vec<u8>)> = vec![
        ("not JSON", b"kind=other risk=low".to_vec()),
        ("empty", Vec::new()),
        ("no answers", b"{}".to_vec()),
        (
            "a kind not offered",
            shaped(json!({"kind": kind("merges"), "risk": kind("low")}))
                .to_string()
                .into_bytes(),
        ),
        (
            "a risk not offered",
            shaped(json!({"kind": kind("none_of_these"), "risk": kind("none")}))
                .to_string()
                .into_bytes(),
        ),
        (
            "an option in the wrong case",
            shaped(json!({"kind": kind("none_of_these"), "risk": kind("LOW")}))
                .to_string()
                .into_bytes(),
        ),
        (
            "a confidence above one",
            shaped(json!({"kind": kind("none_of_these"),
                          "risk": {"type": "choice", "choice": "low", "confidence": 1.5}}))
            .to_string()
            .into_bytes(),
        ),
        (
            "no confidence",
            shaped(
                json!({"kind": kind("none_of_these"), "risk": {"type": "choice", "choice": "low"}}),
            )
            .to_string()
            .into_bytes(),
        ),
        (
            "a score where a choice was asked",
            shaped(json!({"kind": kind("none_of_these"),
                          "risk": {"type": "score", "score": 0.1, "confidence": 0.9}}))
            .to_string()
            .into_bytes(),
        ),
        (
            "only one question",
            shaped(json!({"kind": kind("none_of_these")}))
                .to_string()
                .into_bytes(),
        ),
    ];
    for (what, body) in bodies {
        let r = m.read(200, &body);
        assert!(
            r.answer.is_err(),
            "{:?}: {what} was read as an answer",
            m.provider()
        );
    }
}
