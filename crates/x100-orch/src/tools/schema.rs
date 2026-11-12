//! The JSON schema of each tool's arguments: the serde form of its [`ToolCall`] variant, which is
//! what the arguments are parsed as. Core's types carry no schema of their own, so each argument
//! type has one builder here, and the tests hold every tool's schema to serde with a sample of
//! every variant, so the two cannot drift apart unnoticed.
//!
//! Plain JSON Schema with no references, since harnesses hand the schema to models in their own
//! ways and not all of them follow `$ref`.

use serde::Serialize;
use serde_json::{Map, Value, json};
use x100_core::effort::Effort;
use x100_core::id::{ActionId, AgentId, FileId, JobId, MessageId, TaskId};
use x100_core::spec::{SpecKind, SpecStatus};

use super::Tool;
use proposal::proposal;

/// The schema of `tool`'s arguments, always an object, as MCP requires.
pub(crate) fn schema(tool: Tool) -> Value {
    match tool {
        Tool::Delegate => object(
            &[
                ("to", delegatee()),
                ("task", delegated()),
                ("effort", effort("The effort the task needs.")),
            ],
            &[],
        ),
        Tool::Review => object(
            &[
                (
                    "task",
                    id(TaskId::PREFIX, "A task you delegated that is reported."),
                ),
                ("verdict", verdict()),
            ],
            &[],
        ),
        Tool::Report => object(
            &[
                ("task", id(TaskId::PREFIX, "The task you were given.")),
                ("outcome", agent_outcome()),
                (
                    "summary",
                    text("What was done, for whoever delegated the task."),
                ),
            ],
            &[],
        ),
        Tool::Message => object(
            &[
                ("to", recipient()),
                ("body", text("The message.")),
                ("attachments", files()),
            ],
            &[(
                "in_reply_to",
                id(MessageId::PREFIX, "The message this answers, if any."),
            )],
        ),
        Tool::Wait => object(
            &[
                ("on", wait_on()),
                (
                    "task",
                    id(TaskId::PREFIX, "The task you hold and are waiting in."),
                ),
                ("progress", progress()),
            ],
            &[(
                "deadline",
                timestamp("When to give up waiting. Leave it out for the daemon's default."),
            )],
        ),
        Tool::Schedule => object(&[("when", schedule()), ("does", job_request())], &[]),
        Tool::CancelJob => object(&[("job", id(JobId::PREFIX, "A job you scheduled."))], &[]),
        Tool::RaiseTask => object(
            &[
                ("title", text("A short title.")),
                ("ask", text("What the task asks for.")),
                ("attachments", files()),
            ],
            &[(
                "part",
                text("The part of the project it belongs to, if any."),
            )],
        ),
        Tool::UpdateTask => object(
            &[
                ("task", id(TaskId::PREFIX, "A task you hold.")),
                ("progress", progress()),
            ],
            &[(
                "effort",
                effort("Set only if the task needs a different effort."),
            )],
        ),
        Tool::WriteSpec => object(
            &[
                (
                    "spec",
                    one_of_values(
                        [SpecKind::Brd, SpecKind::Prd, SpecKind::Add],
                        "Which spec: brd, prd or add.",
                    ),
                ),
                (
                    "status",
                    one_of_values(
                        [SpecStatus::Draft, SpecStatus::Approved],
                        "Whether this revision is a draft or approved.",
                    ),
                ),
                ("body", text("The whole document, in Markdown.")),
            ],
            &[],
        ),
        Tool::AddFile => object(
            &[
                ("name", text("The file's name.")),
                ("media_type", text("Its media type, such as image/png.")),
                ("size", count("Its size in bytes.")),
                ("hash", text("The content hash it is stored under.")),
            ],
            &[],
        ),
        Tool::SetPreview => object(&[("url", text("The address to preview."))], &[]),
        Tool::LoadSkill => object(&[("skill", skill_ref())], &[]),
        Tool::StartProcess => object(
            &[
                ("name", text("A name for the process, unique among yours.")),
                ("command", text("The shell command to run.")),
            ],
            &[],
        ),
        Tool::StopProcess => object(&[("name", text("The process's name."))], &[]),
        Tool::Remember => object(
            &[
                ("note", text("The note's name.")),
                ("body", text("The note, in Markdown.")),
            ],
            &[],
        ),
        Tool::FormTeam => object(
            &[(
                "leads",
                array(
                    lead_spec(),
                    "The leads, at least one, each with its own role.",
                ),
            )],
            &[],
        ),
        Tool::SetEffort => object(
            &[
                ("agent", id(AgentId::PREFIX, "The agent.")),
                ("range", effort_range("The range its drivers may run at.")),
            ],
            &[],
        ),
        Tool::SetBudget => object(
            &[
                ("lead", id(AgentId::PREFIX, "The lead.")),
                ("budget", money("Its budget.")),
            ],
            &[],
        ),
        Tool::ReadLog => object(
            &[("from", count("The first event's place in the log."))],
            &[(
                "to",
                count("The last event's place, or leave it out for the end."),
            )],
        ),
        Tool::ReadMemories => object(&[("agent", id(AgentId::PREFIX, "The agent."))], &[]),
        Tool::ProposeChange => {
            let mut s = proposal();
            s["type"] = json!("object");
            s
        }
    }
}

/// An object with `required` fields and `optional` ones, which may also be null, and no others.
fn object(required: &[(&str, Value)], optional: &[(&str, Value)]) -> Value {
    let mut properties = Map::new();
    for (name, schema) in required {
        properties.insert((*name).into(), schema.clone());
    }
    for (name, schema) in optional {
        properties.insert((*name).into(), nullable(schema.clone()));
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        "additionalProperties": false,
    })
}

fn nullable(schema: Value) -> Value {
    let description = schema.get("description").cloned();
    let mut out = json!({ "anyOf": [schema, { "type": "null" }] });
    if let Some(d) = description {
        out["description"] = d;
    }
    out
}

/// An enum's variant by its serde name, holding `inner`: `{"<name>": inner}`.
fn variant(name: &str, inner: Value) -> Value {
    json!({
        "type": "object",
        "properties": { name: inner },
        "required": [name],
        "additionalProperties": false,
    })
}

/// A variant with no data, which serde writes as its bare name.
fn unit(name: &str, description: &str) -> Value {
    json!({ "const": name, "description": description })
}

fn one_of(options: Vec<Value>, description: &str) -> Value {
    json!({ "oneOf": options, "description": description })
}

/// One of these values, each written as serde writes it.
fn one_of_values<T: Serialize, const N: usize>(values: [T; N], description: &str) -> Value {
    let values: Vec<Value> = values
        .iter()
        .map(|v| serde_json::to_value(v).expect("a unit variant serialises"))
        .collect();
    json!({ "type": "string", "enum": values, "description": description })
}

fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn count(description: &str) -> Value {
    json!({ "type": "integer", "minimum": 0, "description": description })
}

fn array(items: Value, description: &str) -> Value {
    json!({ "type": "array", "items": items, "description": description })
}

/// An id as it is written: its kind's prefix, an underscore and a ULID.
fn id(prefix: &str, description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": format!("^{prefix}_[0-9A-HJKMNP-TV-Z]{{26}}$"),
        "description": format!("{description} An id such as {prefix}_01JC6Q4ZB0Q1V7XK4N5W1D8M3E."),
    })
}

fn files() -> Value {
    array(
        id(FileId::PREFIX, "A file of the project's."),
        "Files to attach, by id; often none.",
    )
}

fn timestamp(description: &str) -> Value {
    count(&format!("{description} Milliseconds since the Unix epoch."))
}

fn money(description: &str) -> Value {
    count(&format!(
        "{description} In millionths of a US dollar: 1000000 is one dollar."
    ))
}

fn percent(description: &str) -> Value {
    json!({ "type": "integer", "minimum": 0, "maximum": 100, "description": description })
}

fn effort(description: &str) -> Value {
    one_of_values(Effort::ALL, description)
}

fn effort_range(description: &str) -> Value {
    let mut s = object(
        &[
            ("min", effort("The lowest.")),
            ("max", effort("The highest, not below the lowest.")),
        ],
        &[],
    );
    s["description"] = json!(description);
    s
}

fn progress() -> Value {
    let mut s = object(
        &[(
            "note",
            text("Where the task stands: what is done, what is next, what was decided."),
        )],
        &[(
            "done",
            percent("The share of the task done, if you can say."),
        )],
    );
    s["description"] = json!("Your progress note on the task.");
    s
}

fn party() -> Value {
    one_of(
        vec![
            unit("builder", "The builder."),
            variant("agent", id(AgentId::PREFIX, "An agent.")),
        ],
        "The builder or an agent.",
    )
}

fn recipient() -> Value {
    one_of(
        vec![
            variant("agent", id(AgentId::PREFIX, "Another agent.")),
            unit(
                "builder",
                "The builder. The CEO alone may message the builder.",
            ),
        ],
        "Who the message is for.",
    )
}

fn delegatee() -> Value {
    one_of(
        vec![
            variant("child", id(AgentId::PREFIX, "An agent under you.")),
            variant(
                "new_worker",
                object(
                    &[
                        ("role", text("The new worker's role.")),
                        ("charter", text("What it is for and how it works.")),
                    ],
                    &[(
                        "effort",
                        effort_range(
                            "The CEO alone sets a worker's range; leave it out to give it yours.",
                        ),
                    )],
                ),
            ),
        ],
        "Who takes the task: one of your agents, or a new worker you start.",
    )
}

fn delegated() -> Value {
    one_of(
        vec![
            variant(
                "new",
                object(
                    &[
                        ("title", text("A short title.")),
                        ("ask", text("What the task asks for, in full.")),
                        ("attachments", files()),
                    ],
                    &[],
                ),
            ),
            variant(
                "raised",
                id(TaskId::PREFIX, "A task raised earlier that nobody holds."),
            ),
        ],
        "The task: a new one, or one raised earlier.",
    )
}

fn verdict() -> Value {
    one_of(
        vec![
            unit("accept", "Accept the report and merge the work."),
            variant(
                "send_back",
                object(
                    &[("note", text("What is missing or wrong."))],
                    &[("effort", effort("Set only to ask for more effort."))],
                ),
            ),
        ],
        "Accept, or send the task back.",
    )
}

fn agent_outcome() -> Value {
    one_of(
        vec![
            unit("completed", "The task is done."),
            variant(
                "failed",
                object(&[("reason", text("Why it could not be done."))], &[]),
            ),
        ],
        "How the task ended.",
    )
}

fn wait_on() -> Value {
    one_of(
        vec![
            variant("job", id(JobId::PREFIX, "A job of the project's.")),
            variant("report", id(TaskId::PREFIX, "A task you delegated.")),
            variant(
                "reply",
                object(
                    &[
                        ("message", id(MessageId::PREFIX, "A message you sent.")),
                        ("from", party()),
                    ],
                    &[],
                ),
            ),
            variant(
                "decision",
                id(ActionId::PREFIX, "An action of yours that is held."),
            ),
        ],
        "What to wait for.",
    )
}

fn schedule() -> Value {
    one_of(
        vec![
            variant("once", object(&[("at", timestamp("When it fires."))], &[])),
            variant(
                "every",
                object(
                    &[
                        ("first", timestamp("When it first fires.")),
                        ("every", interval()),
                    ],
                    &[],
                ),
            ),
        ],
        "When the job fires.",
    )
}

/// A [`std::time::Duration`] as serde writes it.
fn interval() -> Value {
    let mut s = object(
        &[
            ("secs", count("Whole seconds.")),
            (
                "nanos",
                count("And nanoseconds, below a second; usually 0."),
            ),
        ],
        &[],
    );
    s["description"] = json!("The time between firings.");
    s
}

fn job_request() -> Value {
    one_of(
        vec![
            variant(
                "wake",
                object(
                    &[
                        ("agent", id(AgentId::PREFIX, "You or one of your agents.")),
                        ("note", text("What the agent is woken with.")),
                    ],
                    &[],
                ),
            ),
            variant(
                "delegate",
                object(
                    &[
                        ("to", id(AgentId::PREFIX, "One of your agents.")),
                        ("title", text("The task's title.")),
                        ("ask", text("What the task asks for.")),
                        ("effort", effort("The effort it needs.")),
                    ],
                    &[],
                ),
            ),
        ],
        "What the job does when it fires.",
    )
}

fn lead_spec() -> Value {
    object(
        &[
            ("role", text("The lead's role, such as Tech.")),
            ("charter", text("What the lead is for and how it works.")),
            ("effort", effort_range("The range its drivers may run at.")),
            ("budget", money("Its budget.")),
        ],
        &[],
    )
}

fn skill_ref() -> Value {
    object(
        &[
            (
                "scope",
                one_of(
                    vec![
                        unit("shared", "Shared in the project."),
                        variant("agent", id(AgentId::PREFIX, "The agent whose own it is.")),
                    ],
                    "Whose skill it is.",
                ),
            ),
            ("name", text("The skill's name.")),
        ],
        &[],
    )
}

mod proposal;

#[cfg(test)]
mod tests;
