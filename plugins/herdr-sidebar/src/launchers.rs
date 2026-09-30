//! Launcher buttons: user-declared commands drawn as icons at the right end of
//! the activity bar, just left of ⚙, so the sidebar doubles as a control pane
//! (open a browser, a plugin manager, a dashboard) without anyone having to
//! remember a keybinding.
//!
//! Declared in `launchers.json` in the plugin's CONFIG dir
//! (`herdr plugin config-dir herdr-sidebar`), which is where herdr says
//! user-editable config belongs. No file means no buttons, which is the
//! sidebar's behavior without this feature. Each machine lists only what it
//! has installed, so there is no "is this tool present?" probing here:
//!
//! ```json
//! [
//!   { "title": "Browser", "icon": "🌐", "nerd_icon": "",
//!     "command": ["~/.local/bin/terminal-browser", "open", "--split", "up", "--size", "0.6"] }
//! ]
//! ```
//!
//! A launched command runs with `HERDR_PANE_ID` / `HERDR_TAB_ID` naming the
//! tab's LARGEST ordinary pane — normally the agent — rather than the sidebar.
//! Pane-aware tools (terminal-browser, herdr's own CLI) split or act relative to
//! `HERDR_PANE_ID`, and the sidebar is the one pane nothing should be opened
//! against. It is read once at startup; `redeploy` picks up an edited file.

use serde::Deserialize;
use std::path::PathBuf;

use crate::icons::IconTheme;

pub const FILE_NAME: &str = "launchers.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launcher {
    /// Shown in the notice line when the button is clicked or fails.
    pub title: String,
    /// Drawn under the Emoji icon theme (and any theme without `nerd_icon`).
    pub icon: String,
    /// Drawn under the Material (Nerd Font) icon theme.
    #[serde(default)]
    pub nerd_icon: Option<String>,
    /// argv, not a shell string. A leading `~/` in the program is expanded, and
    /// a bare `herdr` resolves to the running herdr (`HERDR_BIN_PATH`).
    pub command: Vec<String>,
}

impl Launcher {
    pub fn icon(&self, theme: IconTheme) -> &str {
        match (theme, &self.nerd_icon) {
            (IconTheme::Material, Some(nerd)) => nerd,
            _ => &self.icon,
        }
    }
}

/// The launchers file's path, when herdr gave this process a config dir.
pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("HERDR_PLUGIN_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join(FILE_NAME))
}

/// The declared launchers. An absent file is `Ok(empty)`; an unreadable or
/// malformed one is an `Err` naming the file and the reason, so the caller can
/// say so instead of the buttons silently not appearing.
pub fn load() -> Result<Vec<Launcher>, String> {
    let Some(path) = config_path() else {
        return Ok(Vec::new());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|err| format!("{}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

pub fn parse(json: &str) -> Result<Vec<Launcher>, String> {
    let launchers: Vec<Launcher> = serde_json::from_str(json).map_err(|err| err.to_string())?;
    for launcher in &launchers {
        if launcher
            .command
            .first()
            .is_none_or(|program| program.is_empty())
        {
            return Err(format!(
                "launcher \"{}\" has an empty command",
                launcher.title
            ));
        }
        if launcher.icon.is_empty() {
            return Err(format!("launcher \"{}\" has an empty icon", launcher.title));
        }
    }
    Ok(launchers)
}

/// `(pane, tab)` a launched command should treat as "here": the largest
/// ordinary (non-plugin) pane in the sidebar's own tab, by area. `None` when
/// the sidebar's tab cannot be found or holds nothing but plugin panes; the
/// command then runs without the variables rather than against the sidebar.
pub fn target_pane(
    pane_list_json: &str,
    layout_json: &str,
    my_pane_id: &str,
) -> Option<(String, String)> {
    let tab = tab_of(pane_list_json, my_pane_id)?;
    let candidates = crate::launch::work_panes_in_tab(pane_list_json, &tab);
    let layout: serde_json::Value =
        serde_json::from_str(layout_json.trim_start_matches('\u{feff}')).ok()?;
    let panes = layout.pointer("/result/layout/panes")?.as_array()?;
    let area = |id: &str| {
        panes
            .iter()
            .find(|pane| pane.get("pane_id").and_then(|v| v.as_str()) == Some(id))
            .and_then(|pane| pane.get("rect"))
            .map(|rect| {
                let dim = |key| rect.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
                dim("width") * dim("height")
            })
    };
    let mut best: Option<(&str, i64)> = None;
    for id in &candidates {
        let Some(area) = area(id) else { continue };
        // Strictly greater, so on a tie the earlier pane wins and one layout
        // always yields one target.
        if area > 0 && best.is_none_or(|(_, best_area)| area > best_area) {
            best = Some((id, area));
        }
    }
    best.map(|(pane, _)| (pane.to_string(), tab))
}

fn tab_of(pane_list_json: &str, pane_id: &str) -> Option<String> {
    let value: serde_json::Value =
        serde_json::from_str(pane_list_json.trim_start_matches('\u{feff}')).ok()?;
    value
        .pointer("/result/panes")?
        .as_array()?
        .iter()
        .find(|pane| pane.get("pane_id").and_then(|v| v.as_str()) == Some(pane_id))?
        .get("tab_id")?
        .as_str()
        .map(str::to_string)
}

/// The program to execute for `command[0]`.
fn resolve_program(program: &str) -> PathBuf {
    if let Some(rest) = program.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    if program == "herdr"
        && let Some(bin) = std::env::var_os("HERDR_BIN_PATH").filter(|b| !b.is_empty())
    {
        return PathBuf::from(bin);
    }
    PathBuf::from(program)
}

/// Start the launcher detached from the sidebar's terminal, and reap it on a
/// background thread so it never lingers as a zombie. Only the SPAWN is
/// reported: these commands open panes and popups and return, and nothing
/// here waits on that.
pub fn spawn(launcher: &Launcher, target: Option<(String, String)>) -> Result<(), String> {
    let (program, args) = launcher
        .command
        .split_first()
        .ok_or_else(|| format!("launcher \"{}\" has an empty command", launcher.title))?;
    let mut command = std::process::Command::new(resolve_program(program));
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match target {
        Some((pane, tab)) => {
            command.env("HERDR_PANE_ID", pane).env("HERDR_TAB_ID", tab);
        }
        None => {
            command
                .env_remove("HERDR_PANE_ID")
                .env_remove("HERDR_TAB_ID");
        }
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("{}: could not start {program}: {err}", launcher.title))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_launchers_and_picks_the_icon_per_theme() {
        let launchers = parse(
            r#"[
                {"title": "Browser", "icon": "B", "nerd_icon": "N", "command": ["tb", "open"]},
                {"title": "Plugins", "icon": "P", "command": ["herdr", "plugin"]}
            ]"#,
        )
        .unwrap();
        assert_eq!(launchers.len(), 2);
        assert_eq!(launchers[0].icon(IconTheme::Material), "N");
        assert_eq!(launchers[0].icon(IconTheme::Emoji), "B");
        assert_eq!(
            launchers[1].icon(IconTheme::Material),
            "P",
            "no nerd_icon falls back"
        );
    }

    #[test]
    fn rejects_what_would_render_a_dead_button() {
        assert!(parse(r#"[{"title": "x", "icon": "X", "command": []}]"#).is_err());
        assert!(parse(r#"[{"title": "x", "icon": "X", "command": [""]}]"#).is_err());
        assert!(parse(r#"[{"title": "x", "icon": "", "command": ["a"]}]"#).is_err());
        assert!(
            parse(r#"[{"title": "x", "icon": "X", "command": ["a"], "comand": ["b"]}]"#).is_err(),
            "a misspelled key is an error, not an ignored field"
        );
        assert!(parse("not json").is_err());
        assert_eq!(parse("[]").unwrap(), Vec::new());
    }

    const PANES: &str = r#"{"result":{"panes":[
        {"pane_id":"w1:p1","tab_id":"w1:t1","focused":true,"tokens":{"herdr-sidebar-explorer":"1"}},
        {"pane_id":"w1:p2","tab_id":"w1:t1","focused":false},
        {"pane_id":"w1:p3","tab_id":"w1:t1","focused":false},
        {"pane_id":"w1:p4","tab_id":"w1:t1","focused":false,"tokens":{"herdr-sidebar-preview":"1"}},
        {"pane_id":"w2:p1","tab_id":"w2:t1","focused":false}
    ]}}"#;

    fn layout(rects: &[(&str, i64, i64)]) -> String {
        let panes: Vec<String> = rects
            .iter()
            .map(|(id, w, h)| {
                format!(r#"{{"pane_id":"{id}","rect":{{"width":{w},"height":{h},"x":0,"y":0}}}}"#)
            })
            .collect();
        format!(
            r#"{{"result":{{"layout":{{"panes":[{}]}}}}}}"#,
            panes.join(",")
        )
    }

    #[test]
    fn targets_the_largest_ordinary_pane_in_the_sidebars_own_tab() {
        let layout = layout(&[
            ("w1:p1", 30, 50),  // the sidebar itself
            ("w1:p2", 120, 30), // the agent: largest ordinary pane
            ("w1:p3", 120, 20),
            ("w1:p4", 200, 50), // a preview viewer: larger, but a plugin pane
        ]);
        assert_eq!(
            target_pane(PANES, &layout, "w1:p1"),
            Some(("w1:p2".to_string(), "w1:t1".to_string()))
        );
    }

    #[test]
    fn a_tie_goes_to_the_earlier_pane_and_no_target_is_none() {
        let tie = layout(&[("w1:p2", 100, 10), ("w1:p3", 10, 100)]);
        assert_eq!(
            target_pane(PANES, &tie, "w1:p1").map(|t| t.0),
            Some("w1:p2".into())
        );
        let only_plugins = layout(&[("w1:p1", 30, 50), ("w1:p4", 200, 50)]);
        assert_eq!(target_pane(PANES, &only_plugins, "w1:p1"), None);
        assert_eq!(
            target_pane(PANES, &tie, "w9:p9"),
            None,
            "unknown sidebar pane"
        );
        assert_eq!(target_pane("garbage", &tie, "w1:p1"), None);
    }
}
