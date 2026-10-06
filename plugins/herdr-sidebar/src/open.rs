//! `herdr-sidebar --open <path>[:line]`: show a file in the calling pane's tab
//! from OUTSIDE the sidebar. Written for an agent that wants to put a file in
//! front of the user, so it is run from the agent's own pane and takes its
//! bearings from the `HERDR_PANE_ID` herdr gives every pane.
//!
//! The outcome is the exit status: 0 with one line on stdout when the file is
//! showing, 1 with the reason on stderr when it is not. Nothing is retried and
//! nothing is half-done, so a caller can report a refusal as it stands. The
//! etiquette that separates this from a click lives on `viewer::open_above`.
//!
//! Openers apply here as they do in the Explorer, since a PDF is no more
//! previewable for having been asked for by an agent.

use std::path::{Path, PathBuf};

pub fn run(arg: Option<String>) -> Result<String, String> {
    let arg = arg.ok_or("usage: herdr-sidebar --open <path>[:line]")?;
    let pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .ok_or("not opened: --open must run inside a herdr pane (HERDR_PANE_ID is unset)")?;
    let cwd = std::env::current_dir().map_err(|e| format!("not opened: {e}"))?;
    let (path, line) = resolve(&arg, &cwd)?;

    let openers = crate::openers::load().map_err(|e| format!("not opened: {e}"))?;
    if let Some(opener) = crate::openers::find(&openers, &path) {
        let target = std::env::var("HERDR_TAB_ID")
            .ok()
            .map(|tab_id| (pane_id, tab_id));
        crate::openers::spawn(opener, &path, target).map_err(|e| format!("not opened: {e}"))?;
        return Ok(format!("{}: {}", opener.title, path.display()));
    }

    let payload = match line {
        Some(line) => crate::viewer::file_request_at(&path, line),
        None => crate::viewer::file_request(&path),
    };
    let doc_key = crate::viewer::doc_key_for_file(&path);
    let target = crate::viewer::open_above(&pane_id, &cwd, &doc_key, &payload)?;
    Ok(format!(
        "preview: {} (pane {})",
        path.display(),
        target.pane_id
    ))
}

/// The file `arg` names, absolute, and the line it asks for. A name that
/// exists as written wins over reading a trailing `:N` as a line, so a file
/// really called `notes:3` still opens.
fn resolve(arg: &str, cwd: &Path) -> Result<(PathBuf, Option<usize>), String> {
    let whole = absolute(arg, cwd);
    if whole.is_file() {
        return Ok((whole, None));
    }
    if let Some((head, tail)) = arg.rsplit_once(':')
        && let Ok(line) = tail.parse::<usize>()
        && line > 0
    {
        let file = absolute(head, cwd);
        if file.is_file() {
            return Ok((file, Some(line)));
        }
    }
    Err(if whole.is_dir() {
        format!("not opened: {} is a directory", whole.display())
    } else {
        format!("not opened: no such file: {}", whole.display())
    })
}

/// Absolute without touching the filesystem: the Explorer keys a document by
/// the path it walked to, so resolving symlinks here would open a second copy
/// of a file it already shows.
fn absolute(arg: &str, cwd: &Path) -> PathBuf {
    let joined = cwd.join(arg);
    std::path::absolute(&joined).unwrap_or(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hs-open-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_relative_path_resolves_against_the_callers_directory() {
        let dir = scratch("relative");
        std::fs::write(dir.join("a.rs"), "").unwrap();
        assert_eq!(resolve("a.rs", &dir), Ok((dir.join("a.rs"), None)));
        assert_eq!(resolve("./a.rs:12", &dir), Ok((dir.join("a.rs"), Some(12))));
        let absolute = dir.join("a.rs").display().to_string();
        assert_eq!(
            resolve(&absolute, Path::new("/")),
            Ok((dir.join("a.rs"), None))
        );
    }

    #[test]
    fn a_file_named_like_a_line_reference_opens_as_itself() {
        let dir = scratch("colon");
        std::fs::write(dir.join("notes:3"), "").unwrap();
        assert_eq!(resolve("notes:3", &dir), Ok((dir.join("notes:3"), None)));
    }

    #[test]
    fn what_cannot_be_shown_is_refused_with_the_reason() {
        let dir = scratch("refused");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.rs"), "").unwrap();
        assert!(resolve("sub", &dir).unwrap_err().contains("is a directory"));
        assert!(
            resolve("gone.rs", &dir)
                .unwrap_err()
                .contains("no such file")
        );
        // Line 0 is not a line, and the name with its suffix is not a file.
        assert!(
            resolve("a.rs:0", &dir)
                .unwrap_err()
                .contains("no such file")
        );
    }
}
