//! The schema of `propose_change`'s proposal, which reflection makes.

use serde_json::{Value, json};
use x100_core::id::{AgentId, TaskId};

use super::{array, effort_range, id, lead_spec, money, object, one_of, skill_ref, text, variant};

pub(super) fn proposal() -> Value {
    let skill_change = || {
        object(
            &[
                ("skill", skill_ref()),
                ("body", text("The skill, in Markdown.")),
            ],
            &[],
        )
    };
    let change = one_of(
        vec![
            variant("edit_skill", skill_change()),
            variant("new_skill", skill_change()),
            variant(
                "compact_memory",
                object(
                    &[
                        ("agent", id(AgentId::PREFIX, "Whose memory.")),
                        (
                            "notes",
                            array(
                                json!({
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "minItems": 2,
                                    "maxItems": 2,
                                }),
                                "Each note as its name and its body.",
                            ),
                        ),
                    ],
                    &[],
                ),
            ),
        ],
        "The change to try.",
    );
    let check = one_of(
        vec![
            variant("script", object(&[("command", text("A command."))], &[])),
            variant("rubric", object(&[("text", text("A rubric."))], &[])),
        ],
        "A check.",
    );
    let eval_item = one_of(
        vec![
            variant(
                "past_task",
                object(
                    &[
                        ("task", id(TaskId::PREFIX, "An accepted task.")),
                        ("checkpoint", text("The commit it started at.")),
                    ],
                    &[],
                ),
            ),
            variant("check", check),
            variant(
                "probe",
                object(
                    &[
                        ("question", text("A question.")),
                        ("answer", text("Its answer.")),
                    ],
                    &[],
                ),
            ),
        ],
        "Something to run and score.",
    );
    let eval = object(
        &[
            ("items", array(eval_item, "What to run.")),
            (
                "runs_per_arm",
                json!({ "type": "integer", "minimum": 0, "maximum": 255 }),
            ),
        ],
        &[],
    );
    let agent = |what| id(AgentId::PREFIX, what);
    let team_change = one_of(
        vec![
            variant("add_lead", lead_spec()),
            variant(
                "fold",
                object(
                    &[
                        ("lead", agent("The lead.")),
                        ("into", agent("Its new lead.")),
                    ],
                    &[],
                ),
            ),
            variant(
                "promote",
                object(
                    &[
                        ("worker", agent("The worker.")),
                        ("effort", effort_range("Its range as a lead.")),
                        ("budget", money("Its budget.")),
                    ],
                    &[],
                ),
            ),
            variant("retire", object(&[("agent", agent("The agent."))], &[])),
        ],
        "A change to the team.",
    );
    one_of(
        vec![
            variant("trial", object(&[("change", change), ("eval", eval)], &[])),
            variant(
                "correction",
                object(
                    &[
                        ("agent", agent("Whose note.")),
                        ("note", text("The note's name.")),
                        ("body", text("The note as it should read.")),
                        (
                            "sources",
                            array(
                                json!({ "type": "integer", "minimum": 1 }),
                                "The events in the log that show the note was wrong.",
                            ),
                        ),
                    ],
                    &[],
                ),
            ),
            variant(
                "team",
                object(
                    &[
                        ("changes", array(team_change, "The changes.")),
                        ("telemetry", text("What prompted them.")),
                        ("team_toml", text("The team as it would be.")),
                    ],
                    &[],
                ),
            ),
        ],
        "What to propose.",
    )
}
