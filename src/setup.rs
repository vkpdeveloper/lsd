//! Looking after lsd itself: `--update`, `--uninstall`, and `--completions`
//! with no shell, which sets up tab completion.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::ValueEnum;

use crate::Shell;

const LATEST_RELEASE: &str = "https://api.github.com/repos/vkpdeveloper/lsd/releases/latest";
const INSTALL_SH: &str = "https://raw.githubusercontent.com/vkpdeveloper/lsd/main/install.sh";

/// The script behind `curl .../uninstall.sh | sh`, so both remove the same
/// things and leave LSDeluxe's `lsd` alone.
const UNINSTALL_SH: &str = include_str!("../uninstall.sh");

/// Brackets what `--completions` adds to a shell's config, so the uninstaller
/// can take exactly that out again.
const BEGIN: &str = "# >>> lsd tab completion >>>";
const END: &str = "# <<< lsd tab completion <<<";

pub fn update() -> ExitCode {
    // Before the install replaces this file: on Linux the path then reads
    // "lsd (deleted)".
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
    if !run_script(&script, "install.sh", exe.as_deref()) {
        return ExitCode::FAILURE;
    }
    // Nushell can't run lsd for its completions, so its file holds a copy of
    // them. Write the new lsd's.
    if let (Some(exe), Some(file)) = (exe, nushell_file())
        && file.exists()
        && let Ok(out) = Command::new(exe)
            .args(["--completions", "nushell"])
            .output()
        && out.status.success()
    {
        let _ = fs::write(file, out.stdout);
    }
    ExitCode::SUCCESS
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

/// Sets up tab completion for the shell lsd was run from.
pub fn completions() -> ExitCode {
    let Some(shell) = current_shell() else {
        eprintln!(
            "lsd: couldn't tell which shell this is. Run lsd --completions from bash, zsh, fish, elvish, PowerShell or Nushell."
        );
        return ExitCode::FAILURE;
    };
    match set_up(shell) {
        Ok(done) => {
            println!("{done}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!(
                "lsd: couldn't set up tab completion for {}: {err}",
                name(shell)
            );
            ExitCode::FAILURE
        }
    }
}

/// The shell that ran lsd, so running it in fish sets up fish even when the
/// login shell is zsh. Then the login shell.
fn current_shell() -> Option<Shell> {
    let parent = std::os::unix::process::parent_id().to_string();
    let ps = Command::new("ps")
        .args(["-o", "comm=", "-p", &parent])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned());
    ps.as_deref()
        .and_then(shell_named)
        .or_else(|| shell_named(&std::env::var("SHELL").ok()?))
}

/// A shell from its program's path. Login shells show up as "-zsh".
fn shell_named(path: &str) -> Option<Shell> {
    let program = path.rsplit('/').next()?.trim_start_matches('-');
    Some(match program {
        "bash" => Shell::Bash,
        "zsh" => Shell::Zsh,
        "fish" => Shell::Fish,
        "elvish" => Shell::Elvish,
        "pwsh" | "powershell" => Shell::Powershell,
        "nu" => Shell::Nushell,
        _ => return None,
    })
}

fn name(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => "bash",
        Shell::Zsh => "zsh",
        Shell::Fish => "fish",
        Shell::Elvish => "elvish",
        Shell::Powershell => "PowerShell",
        Shell::Nushell => "Nushell",
    }
}

/// Sets up `shell` and says what it did.
fn set_up(shell: Shell) -> io::Result<String> {
    let home = home()?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let (file, lines) = match shell {
        // macOS terminals start bash as a login shell, which reads
        // .bash_profile and not .bashrc.
        Shell::Bash if cfg!(target_os = "macos") => (
            home.join(".bash_profile"),
            r#"command -v lsd >/dev/null && eval "$(lsd --completions bash)""#,
        ),
        Shell::Bash => (
            home.join(".bashrc"),
            r#"command -v lsd >/dev/null && eval "$(lsd --completions bash)""#,
        ),
        // The script registers itself with compdef, which needs compinit. At
        // the end of .zshrc, oh-my-zsh and the like have run it already.
        Shell::Zsh => (
            std::env::var_os("ZDOTDIR")
                .map(PathBuf::from)
                .unwrap_or(home.clone())
                .join(".zshrc"),
            "if command -v lsd >/dev/null; then\n  (( $+functions[compdef] )) || { autoload -Uz compinit && compinit; }\n  source <(lsd --completions zsh)\nfi",
        ),
        Shell::Elvish => (
            config.join("elvish/rc.elv"),
            "if (has-external lsd) { eval (lsd --completions elvish | slurp) }",
        ),
        Shell::Powershell => (
            powershell_profile()
                .unwrap_or_else(|| config.join("powershell/Microsoft.PowerShell_profile.ps1")),
            "if (Get-Command lsd -ErrorAction SilentlyContinue) { lsd --completions powershell | Out-String | Invoke-Expression }",
        ),
        // fish loads this file when you first complete lsd, and it asks lsd,
        // so it never goes out of date.
        Shell::Fish => {
            let file = config.join("fish/completions/lsd.fish");
            write_file(&file, b"lsd --completions fish | source\n")?;
            return Ok(format!(
                "Set up tab completion for fish in {}.",
                tilde(&file, &home)
            ));
        }
        Shell::Nushell => {
            let file = nushell_file().ok_or_else(|| io::Error::other("couldn't run nu"))?;
            write_file(&file, &crate::completions(Shell::Nushell)?)?;
            return Ok(format!(
                "Set up tab completion for Nushell in {}. lsd --update keeps it current.",
                tilde(&file, &home)
            ));
        }
    };
    let shown = tilde(&file, &home);
    let text = fs::read_to_string(&file).unwrap_or_default();
    let arg = shell.to_possible_value().expect("every shell has a name");
    let flag = format!("lsd --completions {}", arg.get_name());
    if text.contains(BEGIN) || text.contains(&flag) {
        return Ok(format!(
            "Tab completion for {} is already set up in {shown}.",
            name(shell)
        ));
    }
    let gap = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    append(&file, &format!("{gap}{BEGIN}\n{lines}\n{END}\n"))?;
    let reload = match shell {
        Shell::Bash | Shell::Zsh => format!(", or run: source {shown}"),
        _ => String::new(),
    };
    Ok(format!(
        "Set up tab completion for {} in {shown}.\nOpen a new terminal to use it{reload}",
        name(shell)
    ))
}

fn home() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("HOME isn't set"))
}

fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// What `nu` itself says, or nothing when Nushell isn't installed.
fn nushell_file() -> Option<PathBuf> {
    let out = Command::new("nu")
        .args(["-n", "-c", "$nu.data-dir"])
        .output()
        .ok()?;
    let dir = String::from_utf8(out.stdout).ok()?;
    let dir = dir.trim();
    (out.status.success() && !dir.is_empty()).then(|| Path::new(dir).join("vendor/autoload/lsd.nu"))
}

fn powershell_profile() -> Option<PathBuf> {
    let out = Command::new("pwsh")
        .args(["-NoProfile", "-Command", "$PROFILE"])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    (out.status.success() && !path.is_empty()).then(|| PathBuf::from(path))
}

fn write_file(file: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(file, contents)
}

/// Appends without rewriting the file, so a config that is a symlink into a
/// dotfiles repo stays one.
fn append(file: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(text.as_bytes())
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

    #[test]
    fn knows_shells_by_their_program() {
        assert!(matches!(shell_named("-zsh"), Some(Shell::Zsh)));
        assert!(matches!(
            shell_named("/opt/homebrew/bin/fish"),
            Some(Shell::Fish)
        ));
        assert!(matches!(shell_named("pwsh"), Some(Shell::Powershell)));
        assert!(shell_named("lsd").is_none());
    }
}
