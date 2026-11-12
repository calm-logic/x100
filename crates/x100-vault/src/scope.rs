//! Who is asking for a credential, and whether they may have it.

use x100_core::credential::{Credential, Projects, Refusal, Via};
use x100_core::driver::Purpose;
use x100_core::id::{AgentId, DriverId, ProjectId};

/// The driver a use is for. The proxy learns it from the route the request came in on, and the
/// wrapper from the daemon's own record of the calling driver, never from anything the agent
/// says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Requester {
    pub project: ProjectId,
    pub agent: AgentId,
    pub driver: DriverId,
    pub purpose: Purpose,
}

/// Whether `requester` may use `credential` by `via`. A lab driver gets nothing; otherwise the
/// project must be allowed, and the host (for the proxy) or the command (for the wrapper) must be
/// on the credential's list.
pub(crate) fn check(
    credential: &Credential,
    requester: &Requester,
    via: &Via,
) -> Result<(), Refusal> {
    if matches!(requester.purpose, Purpose::Lab { .. }) {
        return Err(Refusal::LabDriver);
    }
    let scope = &credential.scope;
    if let Projects::Only(projects) = &scope.projects
        && !projects.contains(&requester.project)
    {
        return Err(Refusal::ProjectNotAllowed);
    }
    match via {
        Via::Proxy { host } => {
            if scope.hosts.iter().any(|h| host_matches(h, host)) {
                Ok(())
            } else {
                Err(Refusal::HostNotAllowed)
            }
        }
        Via::Wrapper { command } => {
            if scope.commands.contains(command) {
                Ok(())
            } else {
                Err(Refusal::CommandNotAllowed)
            }
        }
    }
}

/// Whether the daemon may send `credential` to `host` in a request it makes itself. Only the
/// host list binds it: such a request is no project's and no driver's, and a credential the
/// daemon keeps for itself is given no project, so that no driver can use it.
pub(crate) fn check_daemon(credential: &Credential, host: &str) -> Result<(), Refusal> {
    if credential.scope.hosts.iter().any(|h| host_matches(h, host)) {
        Ok(())
    } else {
        Err(Refusal::HostNotAllowed)
    }
}

/// Hosts match exactly, ignoring case and a trailing dot. There are no wildcards: a credential
/// names every host it may reach.
fn host_matches(allowed: &str, host: &str) -> bool {
    allowed
        .trim_end_matches('.')
        .eq_ignore_ascii_case(host.trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use x100_core::credential::CredentialScope;
    use x100_core::id::{CredentialRef, TrialId};
    use x100_core::trial::Arm;

    use super::*;

    fn credential(projects: Projects) -> Credential {
        Credential {
            reference: CredentialRef::from_u128(1),
            name: "homebox".into(),
            scope: CredentialScope {
                hosts: BTreeSet::from(["api.homebox.test".to_owned()]),
                commands: BTreeSet::from(["homebox".to_owned()]),
                projects,
            },
        }
    }

    fn requester(purpose: Purpose) -> Requester {
        Requester {
            project: ProjectId::from_u128(7),
            agent: AgentId::from_u128(8),
            driver: DriverId::from_u128(9),
            purpose,
        }
    }

    fn proxy(host: &str) -> Via {
        Via::Proxy { host: host.into() }
    }

    fn wrapper(command: &str) -> Via {
        Via::Wrapper {
            command: command.into(),
        }
    }

    #[test]
    fn each_dimension_is_checked() {
        let all = credential(Projects::All);
        let work = requester(Purpose::Work);
        assert_eq!(check(&all, &work, &proxy("api.homebox.test")), Ok(()));
        assert_eq!(check(&all, &work, &proxy("API.Homebox.test.")), Ok(()));
        assert_eq!(
            check(&all, &work, &proxy("evil.test")),
            Err(Refusal::HostNotAllowed)
        );
        assert_eq!(
            check(&all, &work, &proxy("sub.api.homebox.test")),
            Err(Refusal::HostNotAllowed)
        );
        assert_eq!(check(&all, &work, &wrapper("homebox")), Ok(()));
        assert_eq!(
            check(&all, &work, &wrapper("curl")),
            Err(Refusal::CommandNotAllowed)
        );
        let other = credential(Projects::Only(BTreeSet::from([ProjectId::from_u128(1)])));
        assert_eq!(
            check(&other, &work, &proxy("api.homebox.test")),
            Err(Refusal::ProjectNotAllowed)
        );
        let mine = credential(Projects::Only(BTreeSet::from([ProjectId::from_u128(7)])));
        assert_eq!(check(&mine, &work, &proxy("api.homebox.test")), Ok(()));
    }

    #[test]
    fn the_daemon_is_bound_by_the_hosts_alone() {
        let none = credential(Projects::Only(BTreeSet::new()));
        assert_eq!(check_daemon(&none, "API.homebox.test."), Ok(()));
        assert_eq!(
            check_daemon(&none, "evil.test"),
            Err(Refusal::HostNotAllowed)
        );
        assert_eq!(
            check(&none, &requester(Purpose::Work), &proxy("api.homebox.test")),
            Err(Refusal::ProjectNotAllowed),
            "a credential with no project reaches no driver"
        );
    }

    #[test]
    fn a_lab_driver_gets_nothing() {
        let lab = requester(Purpose::Lab {
            trial: TrialId::from_u128(1),
            arm: Arm::Changed,
            run: 0,
        });
        let all = credential(Projects::All);
        assert_eq!(
            check(&all, &lab, &proxy("api.homebox.test")),
            Err(Refusal::LabDriver)
        );
        assert_eq!(
            check(&all, &lab, &wrapper("homebox")),
            Err(Refusal::LabDriver)
        );
    }
}
