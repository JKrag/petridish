//! The one error type this crate raises. Ported from `installer.py`'s
//! `InstallError`, message strings included — they appear in the README and in
//! `doctor`'s output, so rewording them is a user-visible change.

use std::fmt;

#[derive(Debug)]
pub enum InstallError {
    /// Not macOS. launchd and `~/Library` are the macOS-only surface, so this
    /// aborts before touching anything rather than half-installing
    /// (ARCHITECTURE.md §8.3 D5).
    UnsupportedPlatform(String),
    /// A required binary is not on `PATH`.
    BinaryNotFound(String),
    /// `launchctl` refused both `bootstrap` and `load -w`. Carries both
    /// stderrs, because which one failed and how is the whole diagnostic.
    LaunchctlFailed {
        bootstrap_stderr: String,
        load_stderr: String,
    },
    /// `settings.json` parsed, but has a shape we did not write and cannot
    /// safely edit around. Better to stop than to normalise away data belonging
    /// to another consumer (ARCHITECTURE.md §8.3 D4).
    UnexpectedSettingsShape(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::UnsupportedPlatform(os) => write!(
                f,
                "petridish only supports macOS (launchd); detected {os:?}."
            ),
            // The from-source hint names the crate that actually produces the
            // missing binary — telling someone to `cargo install --path swab`
            // when `petridish` is what is missing sends them in a circle.
            //
            // `--locked` is not decoration: `cargo install` otherwise ignores
            // Cargo.lock and re-resolves, and a yanked transitive `gix`
            // dependency makes that resolution fail outright (README.md,
            // ARCHITECTURE.md §8.1).
            InstallError::BinaryNotFound(name) => {
                let crate_dir = match name.as_str() {
                    "swab" | "swab-hook" => "swab",
                    "petridish" => "petridish-cli",
                    "petri" => "petri",
                    _ => "swab",
                };
                write!(
                    f,
                    "{name:?} not found on PATH. Install it first: \
                     `brew install jkrag/tap/petridish`, or from a checkout \
                     `cargo install --path {crate_dir} --locked` (--locked is \
                     required; see README.md and ARCHITECTURE.md §8.1)."
                )
            }
            InstallError::LaunchctlFailed {
                bootstrap_stderr,
                load_stderr,
            } => write!(
                f,
                "launchctl failed to load the job:\n  bootstrap: {}\n  load -w:   {}",
                bootstrap_stderr.trim(),
                load_stderr.trim()
            ),
            InstallError::UnexpectedSettingsShape(what) => write!(
                f,
                "refusing to edit ~/.claude/settings.json: {what}. \
                 Fix or move the file and re-run; petridish will not rewrite a \
                 shape it did not create."
            ),
            InstallError::Io(e) => write!(f, "{e}"),
            InstallError::Json(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        InstallError::Io(e)
    }
}

impl From<serde_json::Error> for InstallError {
    fn from(e: serde_json::Error) -> Self {
        InstallError::Json(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The from-source hint has to name the crate that builds the missing
    /// binary. Pointing a user at `--path swab` when `petridish` is what is
    /// absent sends them round in a circle.
    #[test]
    fn the_missing_binary_hint_names_the_right_crate() {
        for (bin, want) in [
            ("swab", "--path swab "),
            ("swab-hook", "--path swab "),
            ("petridish", "--path petridish-cli "),
            ("petri", "--path petri "),
        ] {
            let msg = InstallError::BinaryNotFound(bin.into()).to_string();
            assert!(msg.contains(want), "{bin}: expected {want:?} in {msg}");
            assert!(msg.contains("--locked"), "{bin}: {msg}");
        }
    }

    #[test]
    fn the_platform_error_names_the_detected_os() {
        let msg = InstallError::UnsupportedPlatform("linux".into()).to_string();
        assert!(msg.contains("only supports macOS"), "{msg}");
        assert!(msg.contains("linux"), "{msg}");
    }
}
