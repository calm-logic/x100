//! Agents' branches. Each agent works in its own worktree on the branch `x100/<agent>`, which
//! starts from its delegator's branch, or from the project's main branch for the CEO. Work moves up
//! the tree by merging: an accepted task merges the assignee's branch into the delegator's, and the
//! CEO's report on the builder's task merges the CEO's branch into main. Work moves down when a task
//! reaches an agent: its delegator's branch is merged into the agent's first.
//!
//! These block on git, so they run on a blocking thread.

use x100_core::event::ProjectEvent;
use x100_core::git::CommitId;
use x100_core::id::AgentId;
use x100_core::party::Party;
use x100_store::ProjectStore;
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
pub(crate) fn commit(store: &ProjectStore, agent: AgentId) -> Result<(), String> {
    let dir = store.git.worktree_path(agent);
    if !dir.exists() {
        return Ok(());
    }
    store
        .git
        .commit(&dir, &format!("x100: work of {agent}"))
        .map(drop)
        .map_err(|e| format!("could not commit {agent}'s work: {e}"))
}

/// Merges `from`'s branch into `into`. A conflict is a result: the merge is undone and the
/// conflict stated.
pub(crate) fn merge(store: &ProjectStore, from: AgentId, into: Target) -> Result<Merged, String> {
    let (dir, head) = match into {
        Target::Agent(a) => {
            commit(store, a)?;
            (store.git.worktree_path(a), branch(a))
        }
        // The project's checkout, on whatever branch the builder has it.
        Target::Main => (store.git.project_dir().to_path_buf(), "HEAD".to_owned()),
    };
    let from_branch = branch(from);
    let outcome = store
        .git
        .merge(&dir, &from_branch)
        .map_err(|e| format!("merging {from_branch} could not run: {e}"))?;
    Ok(match outcome {
        MergeOutcome::Merged { commit } => Merged::Into(commit),
        MergeOutcome::UpToDate => Merged::Into(head_of(store, &head)?),
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
/// the agent's parent, or the builder for the CEO.
pub(crate) fn ready(
    store: &ProjectStore,
    agent: AgentId,
    home: Party,
    delegators: &[Party],
) -> Result<Ready, String> {
    let path = store.git.worktree_path(agent);
    let mut ready = Ready::default();
    let mut refresh: Vec<Party> = delegators.to_vec();
    if !path.exists() {
        let from = refresh.first().copied().unwrap_or(home);
        let base = branch_of(store, from)?;
        store
            .git
            .create_worktree(agent, &branch(agent), &base)
            .map_err(|e| format!("could not create the worktree: {e}"))?;
        if !refresh.is_empty() {
            ready.events.push(refreshed(store, agent, from)?);
            refresh.remove(0);
        }
    }
    for from in refresh {
        commit(store, agent)?;
        let base = branch_of(store, from)?;
        match store.git.merge(&path, &base) {
            Ok(MergeOutcome::Conflict { files, .. }) => ready.notes.push(format!(
                "Your branch could not take the latest work from {base}: it conflicts in {}. \
                 Work from what you have.",
                files.join(", ")
            )),
            Ok(_) => ready.events.push(refreshed(store, agent, from)?),
            Err(e) => ready.notes.push(format!(
                "Your branch could not take {base}'s latest work: {e}"
            )),
        }
    }
    Ok(ready)
}

fn refreshed(store: &ProjectStore, agent: AgentId, from: Party) -> Result<ProjectEvent, String> {
    Ok(ProjectEvent::BranchRefreshed {
        agent,
        from,
        commit: head_of(store, &branch(agent))?,
    })
}

/// The branch a party's work is on: an agent's own, or for the builder, the project's main branch
/// (whatever the project's checkout has). An agent that never had a worktree has no work of its
/// own, so its delegatee starts from main.
fn branch_of(store: &ProjectStore, party: Party) -> Result<String, String> {
    let main = || {
        store
            .git
            .current_branch(store.git.project_dir())
            .map_err(|e| format!("could not read the project's branch: {e}"))
    };
    match party {
        Party::Builder => main(),
        Party::Agent(a) if store.git.resolve(&branch(a)).is_ok() => Ok(branch(a)),
        Party::Agent(_) => main(),
    }
}

fn head_of(store: &ProjectStore, rev: &str) -> Result<CommitId, String> {
    store
        .git
        .resolve(rev)
        .map_err(|e| format!("could not read {rev}: {e}"))
}

/// What the agent's worktree has changed since `base`, for its start pack.
pub(crate) fn diff_since(store: &ProjectStore, agent: AgentId, base: &CommitId) -> Option<String> {
    let dir = store.git.worktree_path(agent);
    let diff = store.git.diff_since(&dir, base).ok()?;
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
