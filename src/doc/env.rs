//! Environment files (`.env`, `.env.local`, `prod.env`, `.envrc`, ...) hold
//! secrets, so previews show their keys and hide every value until asked.
//! A hidden value is always the same width, so not even its length shows.

/// What a hidden value shows as.
pub const HIDDEN: &str = "••••••••";

/// Whether a file of this name holds environment variables.
pub fn is_env(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    // `.env`, `.env.local`, `.env-prod`, `.env_test`, `.envrc`.
    let dot_env = name
        .strip_prefix(".env")
        .is_some_and(|rest| rest.is_empty() || rest == "rc" || rest.starts_with(['.', '-', '_']));
    // `prod.env`, `app.env.local`, and the other tools' own names.
    dot_env
        || name.ends_with(".env")
        || name.contains(".env.")
        || matches!(name.as_str(), ".flaskenv" | ".dev.vars")
        || name.starts_with(".dev.vars.")
}

/// `text` with every value hidden: `KEY=secret` becomes `KEY=••••••••`.
/// Comments, blank lines and keys stay as they are. A quoted value that runs
/// over several lines, like a private key, is hidden on all of them.
pub fn hide(text: &str) -> String {
    let mut hider = Hider::default();
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        out.push_str(&hider.line(body));
        out.push_str(&line[body.len()..]);
    }
    out
}

/// Hides values line by line, remembering a quote left open.
#[derive(Default)]
pub struct Hider {
    open: Option<char>,
}

impl Hider {
    pub fn line(&mut self, line: &str) -> String {
        if let Some(quote) = self.open {
            if line.contains(quote) {
                self.open = None;
            }
            return if line.trim().is_empty() {
                String::new()
            } else {
                HIDDEN.into()
            };
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return line.into();
        }
        // `KEY=value`, `export KEY=value`, or `KEY: value`.
        let Some(sep) = line.find('=').or_else(|| line.find(": ")) else {
            return line.into();
        };
        let key_end = sep + if line[sep..].starts_with('=') { 1 } else { 2 };
        let value = &line[key_end..];
        let v = value.trim_start();
        if v.is_empty() {
            return line.into();
        }
        if let Some(quote) = v.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))
            && !v[1..].contains(quote)
        {
            self.open = Some(quote);
        }
        let space = &value[..value.len() - v.len()];
        format!("{}{space}{HIDDEN}", &line[..key_end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_env_files_by_name() {
        for name in [
            ".env",
            ".ENV",
            ".env.local",
            ".env.production.local",
            ".env-dev",
            ".env_test",
            ".envrc",
            "prod.env",
            "app.env.local",
            ".flaskenv",
            ".dev.vars",
        ] {
            assert!(is_env(name), "{name}");
        }
        for name in ["environment.ts", ".environ", "env.rs", ".envy", "dotenv.md"] {
            assert!(!is_env(name), "{name}");
        }
    }

    #[test]
    fn hides_values_and_keeps_keys() {
        let src = "# Google\nAPI_KEY=AIzaSyB9v\n\nexport TOKEN = \"abc def\"\nEMPTY=\nURL: http://x\nnot a pair\n";
        assert_eq!(
            hide(src),
            "# Google\nAPI_KEY=••••••••\n\nexport TOKEN = ••••••••\nEMPTY=\nURL: ••••••••\nnot a pair\n"
        );
    }

    #[test]
    fn hides_values_over_several_lines() {
        let src = "KEY=\"-----BEGIN KEY-----\nMIIEvQIBADANBg\n-----END KEY-----\"\nNEXT=1\r\n";
        assert_eq!(
            hide(src),
            "KEY=••••••••\n••••••••\n••••••••\nNEXT=••••••••\r\n"
        );
    }
}
