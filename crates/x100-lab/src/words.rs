//! What a lab driver is told. Its start pack is the agent's own, read with or without the change,
//! under a head that says it is in the lab; each item reaches it as one turn's input. The first
//! line names the trial, the arm and the run, so anything reading the pack, a scripted harness
//! included, can tell a lab run from work.

use x100_core::id::{TaskId, TrialId};
use x100_core::trial::Arm;

/// The tools a lab driver is offered. Anything else it calls is refused and counts as a tool
/// error: in the lab nothing reaches the project.
pub const LAB_TOOLS: &[&str] = &["report", "load_skill", "read_note"];

/// The head of a lab driver's start pack.
pub fn head(trial: TrialId, arm: Arm, run: u8, runs: u8, about: &str) -> String {
    let side = match arm {
        Arm::Baseline => "without the change",
        Arm::Changed => "with the change",
    };
    format!(
        "LAB RUN: trial {trial}, {side}, run {run} of {runs}\n\n\
         You are in x100's lab, in a trial of {about}. Nothing you do here reaches the project: \
         your working directory is a throwaway copy, you hold no credentials, and requests out \
         may read but never write. Work as you would on the real thing. When you are done, call \
         `report` with an outcome and a summary: that is how this run is scored. The tools here \
         are {}.\n",
        LAB_TOOLS
            .iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The section that carries the skill on trial in full, as this arm has it.
pub fn skill_on_trial(name: &str, text: Option<&str>) -> String {
    match text {
        Some(text) => format!("\nTHE SKILL {name}, IN FULL:\n{text}\n"),
        None => format!("\nTHE SKILL {name}: none in this run.\n"),
    }
}

/// A past task, to do again as it was asked. `task` is the id `report` names.
pub fn task(task: TaskId, title: &str, ask: &str) -> String {
    format!(
        "A task to do, as it was asked.\nTask: {task} ({title})\n\n{ask}\n\nWhen it is done, \
         call `report` on task {task} with outcome completed and a summary."
    )
}

/// A probe question, answered from memory. `task` is the id `report` names; it is the lab's own
/// and no task of the project's.
pub fn probe(task: TaskId, question: &str) -> String {
    format!(
        "A question to answer from your memory.\nTask: {task} (a probe)\nQuestion: {question}\n\n\
         Answer it with `report` on task {task}: outcome completed, and the answer as the \
         summary. Your memory index lists your notes; `read_note` opens one."
    )
}

/// What a lab driver is told when a turn ends without a report.
pub const NUDGE: &str = "You have not reported. Finish the work, then call `report`.";

/// The judge's instructions: a rubric, and what a run did.
pub fn judge(task: TaskId, rubric: &str, summary: &str, diff: &str) -> String {
    format!(
        "JUDGE: mark the work below against the rubric, from 0 to 100.\nTask: {task} (judging)\n\n\
         Rubric:\n{rubric}\n\nThe worker's summary:\n{summary}\n\nWhat it changed:\n{diff}\n\n\
         Answer with `report` on task {task}: outcome completed, and a summary that starts with \
         the mark as a whole number."
    )
}

/// The mark at the start of a judge's summary, if it gave one from 0 to 100.
pub fn mark(summary: &str) -> Option<u8> {
    let digits: String = summary
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse::<u8>().ok().filter(|m| *m <= 100)
}
