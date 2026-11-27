//! Looking after lsd itself: `--update` and `--uninstall`.

use std::io;
use std::path::Path;
use std::process::{Command, ExitCode};

const LATEST_RELEASE: &str = "https://api.github.com/repos/vkpdeveloper/lsd/releases/latest";
const INSTALL_SH: &str = "https://raw.githubusercontent.com/vkpdeveloper/lsd/main/install.sh";

/// The script behind `curl .../uninstall.sh | sh`, so both remove the same
/// things and leave LSDeluxe's `lsd` alone.
const UNINSTALL_SH: &str = include_str!("../uninstall.sh");

pub fn update() -> ExitCode {
    let exe = std::env::current_exe().ok();
    let current = env!("CARGO_PKG_VERSION");
    // If GitHub can't say, install anyway: that is what was asked for.
    if let Ok(latest) = latest_version()
        && version(&latest) <= version(current)
    {
        println!("lsd {current} is the latest version.");
        return ExitCode::SUCCESS;
    }
    let script = match fetch(INSTALL_SH) {
        Ok(script) => String::from_utf8_lossy(&script).into_owned(),
        Err(err) => {
            eprintln!("lsd: couldn't download the installer: {err}");
            return ExitCode::FAILURE;
        }
    };
    if run_script(&script, "install.sh", exe.as_deref()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

pub fn uninstall() -> ExitCode {
    let exe = std::env::current_exe().ok();
    if run_script(UNINSTALL_SH, "uninstall.sh", exe.as_deref()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs an installer script. It installs to, or removes from,
/// $LSD_INSTALL_DIR first, so point that at this lsd's folder, which may not
/// be on the PATH.
fn run_script(script: &str, name: &str, exe: Option<&Path>) -> bool {
    let mut sh = Command::new("sh");
    sh.arg("-c").arg(script).arg(name);
    if let Some(dir) = exe.and_then(Path::parent)
        && std::env::var_os("LSD_INSTALL_DIR").is_none()
    {
        sh.env("LSD_INSTALL_DIR", dir);
    }
    match sh.status() {
        Ok(status) => status.success(),
        Err(err) => {
            eprintln!("lsd: couldn't run sh: {err}");
            false
        }
    }
}

/// Downloads with curl, or wget, like install.sh.
fn fetch(url: &str) -> io::Result<Vec<u8>> {
    let curl = Command::new("curl")
        .args(["--proto", "=https", "--tlsv1.2", "-fsSL", url])
        .output();
    let out = match curl {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Command::new("wget").args(["-q", "-O", "-", url]).output()
        }
        other => other,
    };
    match out {
        Ok(out) if out.status.success() => Ok(out.stdout),
        Ok(_) => Err(io::Error::other(format!("couldn't reach {url}"))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Err(io::Error::other("it needs curl or wget"))
        }
        Err(err) => Err(err),
    }
}

fn latest_version() -> io::Result<String> {
    let body = fetch(LATEST_RELEASE)?;
    let release: serde_json::Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
    release["tag_name"]
        .as_str()
        .map(|tag| tag.trim_start_matches('v').to_owned())
        .ok_or_else(|| io::Error::other("no release"))
}

/// "0.1.10" as [0, 1, 10], so it sorts after "0.1.9".
fn version(v: &str) -> Vec<u64> {
    v.split('.').map(|part| part.parse().unwrap_or(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_sort_after() {
        assert!(version("0.1.10") > version("0.1.9"));
        assert!(version("0.2.0") > version("0.1.9"));
        assert!(version("0.1.2") <= version("0.1.2"));
    }
}
