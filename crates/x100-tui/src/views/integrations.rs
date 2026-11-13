//! Settings' Integrations, then the project's Homebox project. Each service says whether it is
//! connected and how, from the daemon's settings; the ways to connect are what the daemon found on
//! its machine, the one it would choose first.

use x100_core::integration::{GitHubWay, HomeboxWay};
use x100_proto::{GitHubMethod, Integration};

use super::Ctx;
use crate::integrations::{Integrations, method};
use crate::page::{BOLD, DIM, GOOD, Page, Setting, Target, WARN, span, spans};

fn choice(page: &mut Page, label: String, about: String, s: Setting) {
    page.row(
        spans(vec![span(format!("    {label}"), BOLD), span(about, DIM)]),
        Target::Setting(s),
    );
}

pub fn section(page: &mut Page, c: &Ctx, ig: &Integrations) {
    let settings = c.d.settings();
    let found = match &ig.found {
        None => {
            page.blank().heading("Integrations");
            page.styled("  Looking for a Homebox and gh on this machine…", DIM);
            None
        }
        Some(Err(e)) => {
            page.blank().heading("Integrations");
            page.text(format!("  The daemon could not say what it finds: {e}"));
            None
        }
        Some(Ok(f)) => {
            page.blank().heading("Integrations");
            Some(f)
        }
    };

    match &settings.homebox {
        Some(h) => {
            let how = match h.way {
                HomeboxWay::Local => "the Homebox on this machine",
                HomeboxWay::Address => "a Homebox elsewhere, with its key",
            };
            let domain = h
                .primary_domain
                .as_ref()
                .map(|d| format!(" Deploys are public under {d}."))
                .unwrap_or_default();
            page.line(spans(vec![
                span("  Homebox  ", BOLD),
                span(format!("Connected to {}, {how}.", h.address), GOOD),
            ]));
            page.paragraph(
                &format!("Its key is in x100's vault and used by x100 alone.{domain}"),
                "    ",
                c.width,
            );
            choice(
                page,
                "Disconnect".into(),
                String::new(),
                Setting::Disconnect(Integration::Homebox),
            );
        }
        None => {
            page.line(spans(vec![
                span("  Homebox  ", BOLD),
                span(
                    "Not connected. Projects cannot be deployed until it is.",
                    WARN,
                ),
            ]));
            let local = found.and_then(|f| f.homebox.local.clone());
            let mut ways = Vec::new();
            if let Some(at) = &local {
                ways.push((
                    HomeboxWay::Local,
                    "Use local".to_owned(),
                    format!("  Homebox is running on this machine, at {at}."),
                    Setting::HomeboxLocal,
                ));
            }
            ways.push((
                HomeboxWay::Address,
                "Host and key".to_owned(),
                "  A Homebox elsewhere, with an API key made there.".to_owned(),
                Setting::HomeboxAddress,
            ));
            // The way the daemon would choose comes first.
            let first = found.map(|f| f.homebox.default_way);
            ways.sort_by_key(|(w, ..)| Some(*w) != first);
            for (_, label, about, s) in ways {
                choice(page, label, about, s);
            }
        }
    }

    match &settings.github {
        Some(g) => {
            let how = match g.way {
                GitHubWay::LocalGh => "through gh on this machine, which keeps its own token",
                GitHubWay::DeviceFlow { .. } => "through GitHub's sign-in",
            };
            page.line(spans(vec![
                span("  GitHub   ", BOLD),
                span(format!("Connected as {}, {how}.", g.account), GOOD),
            ]));
            choice(
                page,
                "Disconnect".into(),
                String::new(),
                Setting::Disconnect(Integration::GitHub),
            );
        }
        None => {
            page.line(spans(vec![
                span("  GitHub   ", BOLD),
                span(
                    "Not connected. A project's code stays on this machine, so Homebox cannot \
                     deploy it.",
                    WARN,
                ),
            ]));
            let ways = found.map(|f| f.github.ways.clone()).unwrap_or_default();
            if found.is_some() && ways.is_empty() {
                page.paragraph(
                    "Install the gh command line and sign in with `gh auth login`, then open \
                     Settings again.",
                    "    ",
                    c.width,
                );
            }
            for way in ways {
                let about = match way {
                    GitHubMethod::LocalGh => "  gh is logged in on this machine.",
                    GitHubMethod::DeviceFlow => "  Approve x100 on GitHub.",
                };
                choice(page, method(way).into(), about.into(), Setting::GitHub(way));
            }
            if let Some((code, at)) = &ig.github_code {
                page.text(format!("    Enter {code} at {at} to let x100 in."));
            }
        }
    }

    homebox_project(page, c, ig);
}

/// The Homebox project this project deploys to, picked from the connected Homebox's managed
/// projects, Unlink and Deploy.
fn homebox_project(page: &mut Page, c: &Ctx, ig: &Integrations) {
    page.blank().heading("Homebox project");
    let connected = c.d.settings().homebox.is_some();
    let link = c.p.project().and_then(|p| p.homebox.clone());
    let said = match (&link, connected) {
        (Some(l), _) => format!(
            "Deploys to {} ({}), {}.",
            l.project_name, l.repo_full_name, l.environment
        ),
        (None, false) => "Not linked. Connect Homebox under Integrations to deploy there.".into(),
        (None, true) => "Not linked. Pick the Homebox project it deploys to, or ask the CEO to \
                         deploy it and x100 makes one."
            .into(),
    };
    page.paragraph(&said, "  ", c.width);
    if connected {
        match ig.projects.as_ref().and_then(|l| l.projects.as_ref()) {
            None => {
                page.styled("    Reading Homebox's projects…", DIM);
            }
            Some(Err(e)) => {
                page.text(format!("    Homebox's projects could not be read: {e}"));
            }
            Some(Ok(all)) => {
                for p in all.iter().filter(|p| p.managed) {
                    let on = link
                        .as_ref()
                        .is_some_and(|l| l.repo_full_name == p.repo_full_name);
                    page.row(
                        spans(vec![
                            span(format!("    [{}] ", if on { "x" } else { " " }), DIM),
                            span(p.name.clone(), BOLD),
                            span(format!("  {}", p.repo_full_name), DIM),
                        ]),
                        Target::Setting(Setting::LinkHomebox(p.repo_full_name.clone())),
                    );
                }
            }
        }
    }
    if link.is_some() {
        choice(page, "Unlink".into(), String::new(), Setting::UnlinkHomebox);
        choice(
            page,
            "Deploy now".into(),
            "  Its main branch, to the linked project.".into(),
            Setting::Deploy,
        );
    }
}
