//! Locating things without ever hardcoding a path (ARCHITECTURE.md §8.3 D1/D2).
//!
//! Every function here takes what it needs as a parameter rather than reading
//! the environment, with exactly one thin wrapper (`resolve_binary`) that does
//! the `PATH` lookup. That is what `installer.py` did too, and it is why its
//! tests never had to mutate `$HOME`: a test passes a scratch directory in and
//! nothing global moves. Keeping the property means the Rust tests can run in
//! parallel.

use crate::error::InstallError;
use std::path::{Path, PathBuf};

/// Reject non-macOS before anything is written (D5).
///
/// Takes the OS name rather than reading `std::env::consts::OS` so it stays a
/// pure function. That is not only for testability: CI compiles this crate on
/// Linux, and a `#[cfg(target_os = "macos")]` gate would make its tests
/// unrunnable there.
pub fn check_platform(os: &str) -> Result<(), InstallError> {
    if os == "macos" {
        Ok(())
    } else {
        Err(InstallError::UnsupportedPlatform(os.to_string()))
    }
}

/// Is `path` a file with any execute bit set? The `shutil.which` test.
#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// Find `name` on a `:`-separated `PATH` string, returning an absolute path.
///
/// **Absolutised, deliberately not canonicalised.** `std::fs::canonicalize`
/// would resolve symlinks, and under Homebrew `/opt/homebrew/bin/swab` is a
/// symlink into `/opt/homebrew/Cellar/petridish/<version>/bin/swab`. Baking the
/// version-stamped Cellar target into the launchd plist means the next
/// `brew upgrade` moves the binary out from under a plist still pointing at the
/// old one, and the daemon silently stops. Keeping the symlink is what makes the
/// plist survive upgrades — and it also matches `installer.py`'s
/// `os.path.abspath`, which never resolved symlinks either, so there is no
/// behavioural divergence from the implementation this replaces.
pub fn resolve_binary_in(name: &str, path_var: &str) -> Result<PathBuf, InstallError> {
    for dir in path_var.split(':').filter(|d| !d.is_empty()) {
        let candidate = Path::new(dir).join(name);
        if is_executable_file(&candidate) {
            return Ok(absolutise(&candidate));
        }
    }
    Err(InstallError::BinaryNotFound(name.to_string()))
}

/// `os.path.abspath`: make absolute against the cwd, without touching symlinks.
fn absolutise(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// `resolve_binary_in` against the process's real `PATH`. The only environment
/// read in this crate.
pub fn resolve_binary(name: &str) -> Result<PathBuf, InstallError> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    resolve_binary_in(name, &path_var)
}

/// xbar's default plugin directory, anchored on `home`.
///
/// SwiftBar's directory is user-configured and cannot be guessed, so xbar's
/// default is the right fallback and `--menubar-plugins-dir` is the escape
/// hatch.
pub fn default_menubar_plugins_dir(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("xbar")
        .join("plugins")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn write_exec(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn check_platform_accepts_macos_and_rejects_everything_else() {
        assert!(check_platform("macos").is_ok());
        let err = check_platform("linux").unwrap_err();
        assert!(err.to_string().contains("only supports macOS"), "got {err}");
    }

    #[test]
    fn resolve_binary_finds_an_executable_on_the_given_path() {
        let tmp = TempDir::new("paths_resolve_found");
        write_exec(&tmp.path, "swab");
        let found = resolve_binary_in("swab", tmp.path.to_str().unwrap()).unwrap();
        assert_eq!(found, tmp.path.join("swab"));
        assert!(found.is_absolute());
    }

    #[test]
    fn resolve_binary_ignores_a_non_executable_file_of_the_right_name() {
        let tmp = TempDir::new("paths_resolve_not_exec");
        std::fs::write(tmp.path.join("swab"), "not executable").unwrap();
        assert!(resolve_binary_in("swab", tmp.path.to_str().unwrap()).is_err());
    }

    #[test]
    fn resolve_binary_reports_the_missing_name() {
        let tmp = TempDir::new("paths_resolve_missing");
        let err = resolve_binary_in("swab-hook", tmp.path.to_str().unwrap()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("swab-hook"), "got {msg}");
        assert!(msg.contains("not found on PATH"), "got {msg}");
    }

    #[test]
    fn resolve_binary_takes_the_first_hit_and_skips_empty_segments() {
        let first = TempDir::new("paths_first");
        let second = TempDir::new("paths_second");
        write_exec(&first.path, "swab");
        write_exec(&second.path, "swab");
        let path_var = format!(
            "{}::{}",
            first.path.to_str().unwrap(),
            second.path.to_str().unwrap()
        );
        assert_eq!(
            resolve_binary_in("swab", &path_var).unwrap(),
            first.path.join("swab")
        );
    }

    /// A Homebrew install puts a symlink in `/opt/homebrew/bin` pointing into a
    /// version-stamped Cellar directory. The resolved path must stay the
    /// symlink: resolving it would bake `.../Cellar/petridish/1.0.0/bin/swab`
    /// into the launchd plist, and the next `brew upgrade` would leave the
    /// daemon pointing at a path that no longer exists.
    #[test]
    fn resolve_binary_keeps_a_symlink_rather_than_resolving_it() {
        let tmp = TempDir::new("paths_symlink");
        let cellar = tmp.path.join("Cellar/petridish/1.0.0/bin");
        std::fs::create_dir_all(&cellar).unwrap();
        write_exec(&cellar, "swab");
        let bin = tmp.path.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(cellar.join("swab"), bin.join("swab")).unwrap();

        let found = resolve_binary_in("swab", bin.to_str().unwrap()).unwrap();
        assert_eq!(
            found,
            bin.join("swab"),
            "must keep the stable symlink, not resolve into the Cellar"
        );
        assert!(!found.to_string_lossy().contains("Cellar"));
    }

    #[test]
    fn menubar_plugins_dir_is_xbars_default_under_the_given_home() {
        assert_eq!(
            default_menubar_plugins_dir(Path::new("/tmp/h")),
            PathBuf::from("/tmp/h/Library/Application Support/xbar/plugins")
        );
    }
}
