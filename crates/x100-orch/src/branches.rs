//! Agents' branches. Each agent works in its own worktree on the branch `x100/<agent>`, which
//! starts from its delegator's branch, or from the project's main branch for the CEO. Work moves up
//! the tree by merging: an accepted task merges the assignee's branch into the delegator's, and the
//! CEO's report on the builder's task merges the CEO's branch into main. Work moves down when a task
//! reaches an agent: its delegator's branch is merged into the agent's first.
//!
//! These block on git, so they run on a blocking thread.

use std::path::Path;

use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::git::CommitId;
use x100_core::id::{AgentId, InboxItemId};
use x100_core::inbox::InboxItem;
use x100_core::party::Party;
use x100_core::task::TaskState;
use x100_store::Git;
use x100_store::git::MergeOutcome;

pub(crate) fn branch(agent: AgentId) -> String {
    format!("x100/{agent}")
}

/// Where accepted work is merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// The delegator's worktree.
    Agent(AgentId),
    /// The project's own checkout, for the builder's tasks.
    Main,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Merged {
    /// The target's branch now holds the work, at this commit.
    Into(CommitId),
    /// Nothing changed, and this says why.
    Conflict(String),
}

/// Commits everything in the agent's worktree to its branch, if it has a worktree and anything to
/// commit. Called when the agent is between turns or blocked on the tool call that needs it, so
/// nothing writes to the worktree meanwhile.
pub(crate) fn commit(git: &Git, agent: AgentId) -> Result<(), String> {
    let dir = git.worktree_path(agent);
    if !dir.exists() {
        return Ok(());
    }
    git.commit(&dir, &format!("x100: work of {agent}"))
        .map(drop)
        .map_err(|e| format!("could not commit {agent}'s work: {e}"))
}

/// Merges `from`'s branch into `into`. A conflict is a result: the merge is undone and the
/// conflict stated.
pub(crate) fn merge(git: &Git, from: AgentId, into: Target) -> Result<Merged, String> {
    let (dir, head) = match into {
        Target::Agent(a) => {
            commit(git, a)?;
            (git.worktree_path(a), branch(a))
        }
        // The project's checkout, on whatever branch the builder has it.
        Target::Main => (git.project_dir().to_path_buf(), "HEAD".to_owned()),
    };
    let from_branch = branch(from);
    let outcome = git
        .merge(&dir, &from_branch)
        .map_err(|e| format!("merging {from_branch} could not run: {e}"))?;
    Ok(match outcome {
        MergeOutcome::Merged { commit } => Merged::Into(commit),
        MergeOutcome::UpToDate => Merged::Into(head_of(git, &head)?),
        MergeOutcome::Conflict { files, detail } => Merged::Conflict(format!(
            "merging {from_branch} conflicts in {}: {detail}",
            files.join(", ")
        )),
    })
}

/// What getting an agent's worktree ready for a turn did.
#[derive(Debug, Default)]
pub(crate) struct Ready {
    /// `BranchRefreshed` for each delegator whose work the agent's branch now holds.
    pub events: Vec<ProjectEvent>,
    /// What the turn must be told: a refresh that conflicted.
    pub notes: Vec<String>,
}

/// Makes sure the agent has a worktree, and brings its branch up to date with each party that
/// delegated it a task in this turn. A new worktree starts from the first of them, or from `home`:
/// the agent's parent, or the builder for the CEO. A project with no code directory yet has only
/// `scratch`, which is made if it is missing.
pub(crate) fn ready(
    git: Option<&Git>,
    scratch: &Path,
    agent: AgentId,
    home: Party,
    delegators: &[Party],
) -> Result<Ready, String> {
    let Some(git) = git else {
        return std::fs::create_dir_all(scratch)
            .map(|()| Ready::default())
            .map_err(|e| format!("could not make the scratch directory: {e}"));
    };
    let path = git.worktree_path(agent);
    let mut ready = Ready::default();
    let mut refresh: Vec<Party> = delegators.to_vec();
    if !path.exists() {
        let from = refresh.first().copied().unwrap_or(home);
        let base = branch_of(git, from)?;
        git.create_worktree(agent, &branch(agent), &base)
            .map_err(|e| format!("could not create the worktree: {e}"))?;
        if !refresh.is_empty() {
            ready.events.push(refreshed(git, agent, from)?);
            refresh.remove(0);
        }
    }
    for from in refresh {
        commit(git, agent)?;
        let base = branch_of(git, from)?;
        match git.merge(&path, &base) {
            Ok(MergeOutcome::Conflict { files, .. }) => ready.notes.push(format!(
                "Your branch could not take the latest work from {base}: it conflicts in {}. \
                 Work from what you have.",
                files.join(", ")
            )),
            Ok(_) => ready.events.push(refreshed(git, agent, from)?),
            Err(e) => ready.notes.push(format!(
                "Your branch could not take {base}'s latest work: {e}"
            )),
        }
    }
    Ok(ready)
}

fn refreshed(git: &Git, agent: AgentId, from: Party) -> Result<ProjectEvent, String> {
    Ok(ProjectEvent::BranchRefreshed {
        agent,
        from,
        commit: head_of(git, &branch(agent))?,
    })
}

/// The branch a party's work is on: an agent's own, or for the builder, the project's main branch
/// (whatever the project's checkout has). An agent that never had a worktree has no work of its
/// own, so its delegatee starts from main.
fn branch_of(git: &Git, party: Party) -> Result<String, String> {
    let main = || {
        git.current_branch(git.project_dir())
            .map_err(|e| format!("could not read the project's branch: {e}"))
    };
    match party {
        Party::Builder => main(),
        Party::Agent(a) if git.resolve(&branch(a)).is_ok() => Ok(branch(a)),
        Party::Agent(_) => main(),
    }
}

fn head_of(git: &Git, rev: &str) -> Result<CommitId, String> {
    git.resolve(rev)
        .map_err(|e| format!("could not read {rev}: {e}"))
}

/// What the agent's worktree has changed since `base`, for its start pack.
pub(crate) fn diff_since(git: &Git, agent: AgentId, base: &CommitId) -> Option<String> {
    let dir = git.worktree_path(agent);
    let diff = git.diff_since(&dir, base).ok()?;
    if diff.files.is_empty() {
        return None;
    }
    let mut text: String = diff
        .files
        .iter()
        .map(|c| format!("{:?} {}\n", c.kind, c.path))
        .collect();
    text.push('\n');
    text.push_str(&diff.patch);
    Some(text)
}

/// Who delegated the tasks among `items` that are in progress with `agent`.
pub(crate) fn delegators(
    state: &ProjectState,
    agent: AgentId,
    items: &[InboxItemId],
) -> Vec<Party> {
    let Some(a) = state.agent(agent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        if let Some(InboxItem::Task { task }) = a.items.get(item)
            && let Some(TaskState::InProgress(asg)) = state.task(*task).map(|t| &t.state)
            && asg.assignee == agent
            && !out.contains(&asg.delegator)
        {
            out.push(asg.delegator);
        }
    }
    out
}
