//! Checking a harness's reported version against what x100 was built and tested with.

/// What x100 knows about one harness's versions.
pub(crate) struct VersionPolicy {
    pub harness: &'static str,
    /// Older than this lacks something x100 needs, so the driver is refused.
    pub minimum: &'static str,
    /// The version the drivers were tested against. Any other version works with a warning.
    pub tested: &'static str,
}

impl VersionPolicy {
    /// `Err` with the reason when the version is too old or cannot be read, otherwise any warning.
    pub fn check(&self, reported: &str) -> Result<Option<String>, String> {
        let Some(version) = parse(reported) else {
            return Err(format!(
                "{} reported a version x100 cannot read: {reported:?}",
                self.harness
            ));
        };
        let minimum = parse(self.minimum).expect("a valid minimum");
        if version < minimum {
            return Err(format!(
                "{} {reported} is older than {}, the oldest x100 can drive",
                self.harness, self.minimum
            ));
        }
        Ok((reported != self.tested).then(|| {
            format!(
                "{} {reported} is not the version x100 was tested with ({}); its output may differ",
                self.harness, self.tested
            )
        }))
    }
}

/// The leading `major.minor.patch` numbers, ignoring any suffix such as ` (Claude Code)`.
fn parse(version: &str) -> Option<Vec<u64>> {
    let core = version.split_whitespace().next()?;
    let core = core.split(['-', '+']).next()?;
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: VersionPolicy = VersionPolicy {
        harness: "Example",
        minimum: "2.1.0",
        tested: "2.1.322",
    };

    #[test]
    fn the_tested_version_passes_silently() {
        assert_eq!(POLICY.check("2.1.322"), Ok(None));
    }

    #[test]
    fn a_newer_version_passes_with_a_warning() {
        let warning = POLICY.check("2.2.0").unwrap().unwrap();
        assert!(warning.contains("2.2.0"), "{warning}");
    }

    #[test]
    fn an_older_or_unreadable_version_is_refused() {
        assert!(POLICY.check("2.0.99").is_err());
        assert!(POLICY.check("garbage").is_err());
        assert!(POLICY.check("2.1.0-beta").unwrap().is_some());
    }
}
