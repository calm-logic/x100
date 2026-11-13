//! The typed calls of Settings' Integrations part and a project's Homebox setting.

use x100_core::id::ProjectId;
use x100_core::integration::HomeboxLink;
use x100_proto::{
    ConnectGitHub, ConnectHomebox, DeployProject, DisconnectIntegration, GitHubConnecting,
    GitHubMethod, HomeboxConnected, HomeboxProjects, Integration, IntegrationList, LinkHomebox,
    ListHomeboxProjects, ListIntegrations, ProjectDeployed, Secret, UnlinkHomebox,
};

use crate::{Client, ClientError};

impl Client {
    pub async fn list_integrations(&self) -> Result<IntegrationList, ClientError> {
        self.request(ListIntegrations {}).await
    }

    /// Connects the Homebox on this machine. The daemon mints its key; nothing passes here.
    pub async fn connect_local_homebox(&self) -> Result<HomeboxConnected, ClientError> {
        self.request(ConnectHomebox::Local).await
    }

    /// Connects a Homebox elsewhere. The key goes to the daemon's vault and is not held here.
    pub async fn connect_homebox_at(
        &self,
        address: impl Into<String>,
        key: impl Into<String>,
    ) -> Result<HomeboxConnected, ClientError> {
        self.request(ConnectHomebox::Address {
            address: address.into(),
            key: Secret::new(key),
        })
        .await
    }

    pub async fn connect_github(&self, way: GitHubMethod) -> Result<GitHubConnecting, ClientError> {
        self.request(ConnectGitHub { way }).await
    }

    pub async fn disconnect_integration(
        &self,
        integration: Integration,
    ) -> Result<(), ClientError> {
        self.request(DisconnectIntegration { integration }).await?;
        Ok(())
    }

    pub async fn homebox_projects(&self) -> Result<HomeboxProjects, ClientError> {
        self.request(ListHomeboxProjects {}).await
    }

    pub async fn link_homebox(
        &self,
        project: ProjectId,
        homebox_project: impl Into<String>,
        environment: Option<String>,
    ) -> Result<HomeboxLink, ClientError> {
        let linked = self
            .request(LinkHomebox {
                project,
                homebox_project: homebox_project.into(),
                environment,
            })
            .await?;
        Ok(linked.link)
    }

    pub async fn unlink_homebox(&self, project: ProjectId) -> Result<(), ClientError> {
        self.request(UnlinkHomebox { project }).await?;
        Ok(())
    }

    /// Deploys the project's main branch, and answers when it is live or has failed.
    pub async fn deploy(
        &self,
        project: ProjectId,
        environment: Option<String>,
    ) -> Result<ProjectDeployed, ClientError> {
        self.request(DeployProject {
            project,
            environment,
        })
        .await
    }
}
