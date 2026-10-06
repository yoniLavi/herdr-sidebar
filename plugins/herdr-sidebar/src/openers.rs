//! Openers: user-declared commands that take over opening files of given
//! extensions, for the types the built-in preview cannot show usefully (a PDF
//! is a "binary file" notice here, and a real viewer elsewhere).
//!
//! Declared in `openers.json` beside `launchers.json`, in the plugin's CONFIG
//! dir, and for the same reason: each machine lists only what it has installed,
//! so nothing here probes for tools. No file means every file previews as
//! before.
//!
//! ```json
//! [
//!   { "title": "PDF viewer", "extensions": ["pdf"],
//!     "command": ["~/.local/bin/open-pdf"] }
//! ]
//! ```
//!
//! The file's absolute path is appended to `command` as one final argument. The
//! command runs exactly as a launcher does (`launchers::spawn_command`): argv
//! rather than a shell string, detached, with `HERDR_PANE_ID` / `HERDR_TAB_ID`
//! naming the tab's main pane so a pane-aware tool opens beside the agent
//! instead of against the sidebar. Read once at startup; `redeploy` picks up an
//! edited file.

use serde::Deserialize;
use std::path::Path;

pub const FILE_NAME: &str = "openers.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opener {
    /// Shown in the notice line when the opener runs or fails.
    pub title: String,
    /// Matched against the file's extension, ignoring case. A leading dot is
    /// accepted and dropped, so `"pdf"` and `".pdf"` mean the same.
    pub extensions: Vec<String>,
    /// argv, not a shell string; resolved exactly like a launcher's command.
    pub command: Vec<String>,
}

/// The declared openers. An absent file is `Ok(empty)`; an unreadable or
/// malformed one is an `Err` naming the file and the reason, so the caller can
/// say so instead of files silently opening the old way.
pub fn load() -> Result<Vec<Opener>, String> {
    let Some(path) = crate::launchers::config_file(FILE_NAME) else {
        return Ok(Vec::new());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|err| format!("{}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

pub fn parse(json: &str) -> Result<Vec<Opener>, String> {
    let mut openers: Vec<Opener> = serde_json::from_str(json).map_err(|err| err.to_string())?;
    for opener in &mut openers {
        if opener
            .command
            .first()
            .is_none_or(|program| program.is_empty())
        {
            return Err(format!("opener \"{}\" has an empty command", opener.title));
        }
        for extension in &mut opener.extensions {
            *extension = extension.trim_start_matches('.').to_ascii_lowercase();
        }
        // An opener that can match nothing is a typo, not a preference.
        if opener.extensions.is_empty() || opener.extensions.iter().any(String::is_empty) {
            return Err(format!(
                "opener \"{}\" has no usable extensions",
                opener.title
            ));
        }
    }
    Ok(openers)
}

/// The opener that claims `path`, by extension. The first match wins, so the
/// file's order is the precedence.
pub fn find<'a>(openers: &'a [Opener], path: &Path) -> Option<&'a Opener> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    openers
        .iter()
        .find(|opener| opener.extensions.contains(&extension))
}

/// Start `opener` on `path`. Only the SPAWN is reported, as for a launcher.
pub fn spawn(opener: &Opener, path: &Path, target: Option<(String, String)>) -> Result<(), String> {
    crate::launchers::spawn_command(&opener.title, &opener.command, Some(path), target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_extension_ignoring_case_and_a_leading_dot() {
        let openers = parse(
            r#"[
                {"title": "PDF", "extensions": [".PDF"], "command": ["open-pdf"]},
                {"title": "Docs", "extensions": ["pdf", "epub"], "command": ["reader"]}
            ]"#,
        )
        .unwrap();
        assert_eq!(openers[0].extensions, vec!["pdf"]);
        let title = |name: &str| find(&openers, Path::new(name)).map(|o| o.title.as_str());
        assert_eq!(title("/a/Report.PDF"), Some("PDF"), "first match wins");
        assert_eq!(title("/a/book.epub"), Some("Docs"));
        assert_eq!(title("/a/notes.txt"), None);
        assert_eq!(title("/a/pdf"), None, "a name is not an extension");
        assert_eq!(title("/a/archive.pdf.gz"), None, "only the last extension");
    }

    #[test]
    fn rejects_what_could_never_open_anything() {
        let bad = [
            r#"[{"title": "x", "extensions": ["pdf"], "command": []}]"#,
            r#"[{"title": "x", "extensions": ["pdf"], "command": [""]}]"#,
            r#"[{"title": "x", "extensions": [], "command": ["a"]}]"#,
            r#"[{"title": "x", "extensions": ["."], "command": ["a"]}]"#,
            r#"[{"title": "x", "extensions": ["pdf"], "command": ["a"], "extension": "b"}]"#,
            "not json",
        ];
        for json in bad {
            assert!(parse(json).is_err(), "accepted: {json}");
        }
        assert_eq!(parse("[]").unwrap(), Vec::new());
    }

    #[cfg(unix)]
    #[test]
    fn the_path_arrives_as_one_final_argument() {
        let dir = std::env::temp_dir().join(format!("hs-opener-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("argv");
        // A path with a space and a quote: it must not be split or re-parsed.
        let file = dir.join("my \"q\" file.pdf");
        let opener = Opener {
            title: "t".into(),
            extensions: vec!["pdf".into()],
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                r#"printf '%s|%s|%s' "$#" "$1" "$2" > "$0""#.into(),
                out.to_string_lossy().into_owned(),
                "fixed".into(),
            ],
        };
        spawn(&opener, &file, None).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got = String::new();
        while std::time::Instant::now() < deadline {
            got = std::fs::read_to_string(&out).unwrap_or_default();
            if got.ends_with(".pdf") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, format!("2|fixed|{}", file.display()));
    }
}
