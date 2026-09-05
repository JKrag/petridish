//! Talking to `launchctl`, behind a seam tests can record.
//!
//! `installer.py` injected a `runner` callable for the same reason: the retry
//! logic below has three distinct paths, and the only way to prove all three
//! without a real launchd is to assert on the exact argv sequence.

use crate::error::InstallError;
use std::io::Write;
use std::path::Path;

/// What one `launchctl` invocation reported back.
#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub code: i32,
    pub stderr: String,
}

/// The seam. One method, so a recording implementation is a dozen lines.
pub trait Launchctl {
    fn run(&self, args: &[&str]) -> CmdOutput;
}

pub struct RealLaunchctl;

impl Launchctl for RealLaunchctl {
    fn run(&self, args: &[&str]) -> CmdOutput {
        // `installer.py` passed `timeout=10` here. `std::process::Command` has
        // no wait-with-timeout, and the alternatives — a watchdog thread, or a
        // dependency — both cost more than the problem is worth: a hung
        // `launchctl` has never been observed, and this runs interactively
        // where Ctrl-C already works. Dropped deliberately, noted here rather
        // than silently.
        match std::process::Command::new("launchctl").args(args).output() {
            Ok(out) => CmdOutput {
                code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(e) => CmdOutput {
                code: -1,
                stderr: e.to_string(),
            },
        }
    }
}

/// launchctl exit code 5, `EALREADY`.
const EALREADY: i32 = 5;
/// launchctl exit code 3, `ESRCH` — no such job, i.e. already not loaded.
const ESRCH: i32 = 3;

/// Register the plist with launchd, replacing any stale registration.
///
/// `EALREADY` means launchd already holds a job under this label — but from
/// *its own in-memory definition*, which may point at a binary that has since
/// moved (a `brew upgrade`, a `cargo install` into a different prefix). Simply
/// returning on 5 leaves that stale definition running forever: every
/// subsequent reinstall rewrites the plist on disk while launchd keeps
/// executing the old program path. Booting the stale job out first is what
/// makes the fresh plist actually take effect.
pub fn load_job(
    plist_path: &Path,
    uid: u32,
    label: &str,
    ctl: &dyn Launchctl,
) -> Result<(), InstallError> {
    let domain = format!("gui/{uid}");
    let plist = plist_path.to_string_lossy().into_owned();

    let mut result = ctl.run(&["bootstrap", &domain, &plist]);
    if result.code == 0 {
        return Ok(());
    }

    if result.code == EALREADY {
        let service = format!("gui/{uid}/{label}");
        ctl.run(&["bootout", &service]);
        result = ctl.run(&["bootstrap", &domain, &plist]);
        if result.code == 0 {
            return Ok(());
        }
    }

    // Fallback for domains where bootstrap semantics differ.
    let fallback = ctl.run(&["load", "-w", &plist]);
    if fallback.code != 0 {
        return Err(InstallError::LaunchctlFailed {
            bootstrap_stderr: result.stderr,
            load_stderr: fallback.stderr,
        });
    }
    Ok(())
}

/// Unregister the job, tolerating "not loaded".
///
/// Never returns an error: the plist file is about to be deleted either way,
/// and a stale registration pointing at a missing plist is harmless noise macOS
/// clears at next login. Failing here would abort an uninstall over something
/// that does not matter, so it warns to `warn` and continues.
pub fn unload_job(label: &str, uid: u32, ctl: &dyn Launchctl, warn: &mut dyn Write) {
    let service = format!("gui/{uid}/{label}");
    let result = ctl.run(&["bootout", &service]);
    if result.code == 0 || result.code == ESRCH {
        return;
    }

    let fallback = ctl.run(&["remove", label]);
    if fallback.code != 0 {
        let detail = if result.stderr.trim().is_empty() {
            fallback.stderr.trim()
        } else {
            result.stderr.trim()
        };
        let _ = writeln!(
            warn,
            "warning: launchctl could not unload {label} cleanly ({detail})"
        );
    }
}

#[cfg(test)]
pub mod recording {
    use super::*;
    use std::cell::RefCell;

    /// Records every argv it is handed and replays a scripted list of exit
    /// codes, one per call, defaulting to 0 once the script runs out.
    pub struct RecordingLaunchctl {
        pub calls: RefCell<Vec<Vec<String>>>,
        script: RefCell<Vec<i32>>,
    }

    impl RecordingLaunchctl {
        pub fn new(script: &[i32]) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                script: RefCell::new(script.to_vec()),
            }
        }

        pub fn argv(&self) -> Vec<Vec<String>> {
            self.calls.borrow().clone()
        }
    }

    impl Launchctl for RecordingLaunchctl {
        fn run(&self, args: &[&str]) -> CmdOutput {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|s| s.to_string()).collect());
            let mut script = self.script.borrow_mut();
            let code = if script.is_empty() {
                0
            } else {
                script.remove(0)
            };
            CmdOutput {
                code,
                stderr: if code == 0 {
                    String::new()
                } else {
                    format!("launchctl exited {code}")
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::recording::RecordingLaunchctl;
    use super::*;

    const LABEL: &str = "com.petridish.daemon";
    const UID: u32 = 501;

    fn plist() -> &'static Path {
        Path::new("/Users/x/Library/LaunchAgents/com.petridish.daemon.plist")
    }

    #[test]
    fn a_clean_bootstrap_is_a_single_call() {
        let ctl = RecordingLaunchctl::new(&[0]);
        load_job(plist(), UID, LABEL, &ctl).unwrap();
        assert_eq!(
            ctl.argv(),
            vec![vec![
                "bootstrap".to_string(),
                "gui/501".to_string(),
                plist().to_string_lossy().into_owned()
            ]]
        );
    }

    /// The regression guard. Treating EALREADY as success left launchd running
    /// a stale in-memory job definition that no amount of reinstalling would
    /// replace, so the exact three-call sequence is the thing under test — not
    /// merely that the call succeeded.
    #[test]
    fn ealready_boots_the_stale_job_out_and_bootstraps_again() {
        let ctl = RecordingLaunchctl::new(&[5, 0, 0]);
        load_job(plist(), UID, LABEL, &ctl).unwrap();
        let calls = ctl.argv();
        assert_eq!(
            calls.len(),
            3,
            "expected bootstrap, bootout, bootstrap: {calls:?}"
        );
        assert_eq!(calls[0][0], "bootstrap");
        assert_eq!(calls[1], vec!["bootout", "gui/501/com.petridish.daemon"]);
        assert_eq!(calls[2][0], "bootstrap");
    }

    #[test]
    fn a_failed_bootstrap_falls_back_to_load_minus_w() {
        let ctl = RecordingLaunchctl::new(&[1, 0]);
        load_job(plist(), UID, LABEL, &ctl).unwrap();
        let calls = ctl.argv();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1][0], "load");
        assert_eq!(calls[1][1], "-w");
    }

    #[test]
    fn both_paths_failing_reports_both_stderrs() {
        let ctl = RecordingLaunchctl::new(&[1, 1]);
        let err = load_job(plist(), UID, LABEL, &ctl).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bootstrap:"), "{msg}");
        assert!(msg.contains("load -w:"), "{msg}");
    }

    #[test]
    fn unload_tolerates_esrch_without_a_fallback_call() {
        let ctl = RecordingLaunchctl::new(&[ESRCH]);
        let mut warn = Vec::new();
        unload_job(LABEL, UID, &ctl, &mut warn);
        assert_eq!(
            ctl.argv().len(),
            1,
            "ESRCH means already unloaded; stop there"
        );
        assert!(warn.is_empty());
    }

    #[test]
    fn unload_falls_back_to_remove_and_warns_without_failing() {
        let ctl = RecordingLaunchctl::new(&[1, 1]);
        let mut warn = Vec::new();
        unload_job(LABEL, UID, &ctl, &mut warn);
        let calls = ctl.argv();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1], vec!["remove", LABEL]);
        let text = String::from_utf8(warn).unwrap();
        assert!(text.contains("could not unload"), "{text}");
    }

    #[test]
    fn a_successful_remove_fallback_warns_about_nothing() {
        let ctl = RecordingLaunchctl::new(&[1, 0]);
        let mut warn = Vec::new();
        unload_job(LABEL, UID, &ctl, &mut warn);
        assert!(warn.is_empty());
    }
}
