//! The project and tab last open, kept beside the daemon's socket so the next run opens on them.

use std::path::PathBuf;
use std::str::FromStr;

use x100_core::id::ProjectId;

#[derive(Debug, Clone)]
pub struct Remember {
    file: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remembered {
    pub project: Option<ProjectId>,
    pub tab: Option<String>,
}

impl Remember {
    pub fn beside(socket: &std::path::Path) -> Remember {
        Remember {
            file: socket.parent().map(|d| d.join("x100-tui.state")),
        }
    }

    pub fn none() -> Remember {
        Remember { file: None }
    }

    pub fn read(&self) -> Remembered {
        let mut r = Remembered::default();
        let Some(text) = self
            .file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
        else {
            return r;
        };
        for line in text.lines() {
            match line.split_once('=') {
                Some(("project", v)) => r.project = ProjectId::from_str(v.trim()).ok(),
                Some(("tab", v)) => r.tab = Some(v.trim().to_owned()),
                _ => {}
            }
        }
        r
    }

    /// Best effort: a state that cannot be kept only means the next run opens on the list.
    pub fn write(&self, project: Option<ProjectId>, tab: &str) {
        let Some(file) = &self.file else { return };
        let text = match project {
            Some(p) => format!("project={p}\ntab={tab}\n"),
            None => String::new(),
        };
        let _ = std::fs::write(file, text);
    }
}
