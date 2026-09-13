//! Talking to `systemctl --user`, behind a seam tests can record.

use crate::error::InstallError;
use std::io::Write;

/// What one `systemctl` invocation reported back.
#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub code: i32,
    pub stderr: String,
}

/// The seam. One method, so a recording implementation is a dozen lines.
pub trait Systemctl {
    fn run(&self, args: &[&str]) -> CmdOutput;
}

pub struct RealSystemctl;

impl Systemctl for RealSystemctl {
    fn run(&self, args: &[&str]) -> CmdOutput {
        match std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()
        {
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

/// Write the unit files to disk *before* calling this, then bring the timer up
/// with the fresh definition every time — `daemon-reload` so systemd re-reads
/// the files just written, `enable` so the timer survives reboot (idempotent:
/// re-enabling an already-enabled unit succeeds), then `restart` so a changed
/// binary path or schedule takes effect immediately even if the timer was
/// already running under a stale definition. `restart` on a stopped unit is
/// equivalent to `start`, so this one call covers "never installed",
/// "installed but stopped", and "installed and running" alike.
pub fn enable_and_start_timer(unit: &str, ctl: &dyn Systemctl) -> Result<(), InstallError> {
    let reload = ctl.run(&["daemon-reload"]);
    if reload.code != 0 {
        return Err(InstallError::SystemctlFailed {
            operation: "daemon-reload".to_string(),
            stderr: reload.stderr,
        });
    }
    let enable = ctl.run(&["enable", unit]);
    if enable.code != 0 {
        return Err(InstallError::SystemctlFailed {
            operation: format!("enable {unit}"),
            stderr: enable.stderr,
        });
    }
    let restart = ctl.run(&["restart", unit]);
    if restart.code != 0 {
        return Err(InstallError::SystemctlFailed {
            operation: format!("restart {unit}"),
            stderr: restart.stderr,
        });
    }
    Ok(())
}

/// Tear the timer down, tolerating "was never installed". Never returns an
/// error: the unit files are about to be deleted either way, and a failed
/// `disable --now` on an already-absent unit is harmless noise — warns instead
/// of aborting an uninstall over something that does not matter (mirrors
/// `launchd::unload_job`'s philosophy).
pub fn disable_and_stop_timer(unit: &str, ctl: &dyn Systemctl, warn: &mut dyn Write) {
    let result = ctl.run(&["disable", "--now", unit]);
    if result.code != 0 {
        let _ = writeln!(
            warn,
            "warning: systemctl could not disable {unit} cleanly ({})",
            result.stderr.trim()
        );
    }
}

#[cfg(test)]
pub mod recording {
    use super::*;
    use std::cell::RefCell;

    /// Records every argv it is handed and replays a scripted list of exit
    /// codes, one per call, defaulting to 0 once the script runs out.
    pub struct RecordingSystemctl {
        pub calls: RefCell<Vec<Vec<String>>>,
        script: RefCell<Vec<i32>>,
    }

    impl RecordingSystemctl {
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

    impl Systemctl for RecordingSystemctl {
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
                    format!("systemctl exited {code}")
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::recording::RecordingSystemctl;
    use super::*;

    const UNIT: &str = "petridish-scan.timer";

    #[test]
    fn enable_and_start_timer_calls_reload_enable_restart_in_order() {
        let ctl = RecordingSystemctl::new(&[0, 0, 0]);
        enable_and_start_timer(UNIT, &ctl).unwrap();
        assert_eq!(
            ctl.argv(),
            vec![
                vec!["daemon-reload".to_string()],
                vec!["enable".to_string(), UNIT.to_string()],
                vec!["restart".to_string(), UNIT.to_string()],
            ]
        );
    }

    #[test]
    fn enable_and_start_timer_is_idempotent_on_a_second_call() {
        let ctl = RecordingSystemctl::new(&[0, 0, 0, 0, 0, 0]);
        enable_and_start_timer(UNIT, &ctl).unwrap();
        enable_and_start_timer(UNIT, &ctl).unwrap();
        let calls = ctl.argv();
        assert_eq!(calls.len(), 6, "expected two full rounds: {calls:?}");
    }

    #[test]
    fn a_failed_daemon_reload_stops_before_enabling() {
        let ctl = RecordingSystemctl::new(&[1]);
        let err = enable_and_start_timer(UNIT, &ctl).unwrap_err();
        assert_eq!(ctl.argv().len(), 1, "should only call daemon-reload");
        let msg = err.to_string();
        assert!(msg.contains("daemon-reload"), "{msg}");
    }

    #[test]
    fn a_failed_enable_stops_before_restarting() {
        let ctl = RecordingSystemctl::new(&[0, 1]);
        let err = enable_and_start_timer(UNIT, &ctl).unwrap_err();
        assert_eq!(ctl.argv().len(), 2, "should call daemon-reload and enable");
        let msg = err.to_string();
        assert!(msg.contains("enable"), "{msg}");
    }

    #[test]
    fn a_failed_restart_is_reported() {
        let ctl = RecordingSystemctl::new(&[0, 0, 1]);
        let err = enable_and_start_timer(UNIT, &ctl).unwrap_err();
        assert_eq!(ctl.argv().len(), 3, "all three calls should be made");
        let msg = err.to_string();
        assert!(msg.contains("restart"), "{msg}");
    }

    #[test]
    fn disable_and_stop_timer_never_returns_an_error_even_when_systemctl_fails() {
        let ctl = RecordingSystemctl::new(&[1]);
        let mut warn = Vec::new();
        disable_and_stop_timer(UNIT, &ctl, &mut warn);
        let text = String::from_utf8(warn).unwrap();
        assert!(text.contains("could not disable"), "{text}");
    }

    #[test]
    fn disable_and_stop_timer_is_quiet_on_success() {
        let ctl = RecordingSystemctl::new(&[0]);
        let mut warn = Vec::new();
        disable_and_stop_timer(UNIT, &ctl, &mut warn);
        assert!(warn.is_empty());
    }
}
