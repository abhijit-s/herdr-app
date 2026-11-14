//! Mods: folders of JavaScript under ~/.config/herdr-app/mods that agents write and
//! Herdr runs live (gpui-shell), plus the `herdr` module they import.

use crate::layout::Node;
use gpui::App;
use gpui_shell::{Capabilities, ExecuteGrant, HostError, HostModule, HostObject, HostValue, NetworkGrant};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::{Rc, Weak};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

include!(concat!(env!("OUT_DIR"), "/default_mods.rs"));
include!(concat!(env!("OUT_DIR"), "/frame_mods.rs"));

#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod lifecycle_tests;

/// The frame's own mods (welcome, ...): the app's, not the person's. Written
/// from the binary on every start, so no edit or broken mod can break them.
/// Loaded as `frame-<name>`.
pub fn frame_dir() -> PathBuf {
    app_dir().join("frame")
}

pub fn home() -> PathBuf {
    // HOME, or the profile folder on Windows.
    std::env::home_dir().unwrap_or_else(|| "/".into())
}

/// The fonts on this machine (`fonts()`): read when the window opens and
/// again when the home brings new ones.
static FONTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn note_fonts(cx: &gpui::App) {
    // Names starting with "." are the system's private UI fonts (macOS).
    let mut names: Vec<String> = cx.text_system().all_font_names().into_iter().filter(|n| !n.starts_with('.')).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    *FONTS.lock().unwrap() = names;
}

/// The app's home: `$HERDR_APP_HOME` (tests, experiments), else
/// `$XDG_CONFIG_HOME/herdr-app`, else `~/.config/herdr-app`, on every OS.
pub fn app_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    env("HERDR_APP_HOME").unwrap_or_else(|| env("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config")).join("herdr-app"))
}

pub fn mods_dir() -> PathBuf {
    app_dir().join("mods")
}

const AGENTS_MD: &str = include_str!("../defaults/AGENTS.md");

/// Writes the agent guide and API types, and installs or updates the default
/// mods. Shipped mods are replaced whole; edits are first moved to a clone.
pub fn write_workspace() -> std::io::Result<()> {
    let dir = app_dir();
    std::fs::create_dir_all(mods_dir())?;
    write_if_changed(&dir.join("AGENTS.md"), AGENTS_MD)?;
    // Agents read AGENTS.md now; the pointer file we used to write goes (only ours).
    if std::fs::read_to_string(dir.join("CLAUDE.md")).is_ok_and(|s| s == "@AGENTS.md\n") {
        let _ = std::fs::remove_file(dir.join("CLAUDE.md"));
    }
    write_if_changed(&dir.join("herdr.d.ts"), DECLARATIONS)?;
    let types = gpui_shell::type_declarations(&gpui_shell::FrozenComponentRegistry::default());
    write_if_changed(&dir.join("gpui-kit.d.ts"), &types)?;
    let _ = std::fs::remove_dir_all(frame_dir());
    for (path, bytes) in FRAME_MODS {
        let file = frame_dir().join(path);
        std::fs::create_dir_all(file.parent().unwrap())?;
        std::fs::write(&file, bytes)?;
    }
    sync_defaults(&mods_dir(), &dir.join("installed.json"), DEFAULT_MODS)
}

static SYNC_NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn take_sync_notes() -> Vec<String> {
    std::mem::take(&mut *SYNC_NOTES.lock().unwrap())
}

fn edited_note(clone: &str) {
    SYNC_NOTES.lock().unwrap().push(clone.to_owned());
}

fn edited_name(parent: &Path, name: &str, suffix: &str) -> String {
    (1..).map(|i| if i == 1 { format!("{name}-edited") } else { format!("{name}-edited-{i}") })
        .find(|n| !parent.join(format!("{n}{suffix}")).exists()).unwrap()
}

fn sync_defaults(mods: &Path, installed_path: &Path, shipped_mods: &[(&str, &[u8])]) -> std::io::Result<()> {
    let text = std::fs::read_to_string(installed_path).unwrap_or_default();
    let mut installed: Installed = serde_json::from_str(&text).unwrap_or_default();
    let names: std::collections::BTreeSet<&str> = shipped_mods.iter().filter_map(|(p, _)| p.split('/').next()).collect();
    let dropped: Vec<String> = installed.keys().filter(|n| !n.starts_with("layout:") && !names.contains(n.as_str())).cloned().collect();
    for name in dropped {
        let target = mods.join(&name);
        if installed.get(&name).is_some_and(|was| target.exists() && as_shipped(&target, was)) {
            std::fs::remove_dir_all(&target)?;
        }
        installed.remove(&name);
    }
    for name in names {
        let target = mods.join(name);
        let shipped: Files = shipped_mods.iter()
            .filter_map(|(p, bytes)| Some((p.strip_prefix(&format!("{name}/"))?.to_owned(), fingerprint(bytes))))
            .collect();
        if installed.get(name) == Some(&shipped) && target.exists() && as_shipped(&target, &shipped) {
            continue;
        }
        if target.exists() {
            if !as_shipped(&target, installed.get(name).unwrap_or(&shipped)) {
                let clone = edited_name(mods, name, "");
                std::fs::rename(&target, mods.join(&clone))?;
                edited_note(&clone);
            } else {
                std::fs::remove_dir_all(&target)?;
            }
        }
        for (path, bytes) in shipped_mods.iter().filter(|(p, _)| p.starts_with(&format!("{name}/"))) {
            let file = mods.join(path);
            std::fs::create_dir_all(file.parent().unwrap())?;
            std::fs::write(&file, bytes)?;
        }
        installed.insert(name.to_owned(), shipped);
    }
    write_if_changed(installed_path, serde_json::to_string_pretty(&installed).unwrap() + "\n")
}

pub fn sync_layouts(home: &Path, layouts: &[(&str, &str)]) -> std::io::Result<()> {
    let record = home.join("installed.json");
    let mut installed: Installed = serde_json::from_str(&std::fs::read_to_string(&record).unwrap_or_default()).unwrap_or_default();
    let dir = home.join("layouts");
    std::fs::create_dir_all(&dir)?;
    if dir.join(".base").exists() {
        std::fs::remove_dir_all(dir.join(".base"))?;
    }
    let dropped: Vec<String> = installed.keys().filter(|k| k.strip_prefix("layout:").is_some_and(|n| !layouts.iter().any(|(name, _)| *name == n))).cloned().collect();
    for key in dropped {
        let path = dir.join(format!("{}.json", &key["layout:".len()..]));
        if path.exists() && installed[&key].get("json").is_some_and(|hash| std::fs::read_to_string(&path).is_ok_and(|text| layout_fingerprint(&text) == *hash)) {
            std::fs::remove_file(path)?;
            let _ = std::fs::remove_file(dir.join(".state").join(format!("{}.json", &key["layout:".len()..])));
        }
        installed.remove(&key);
    }
    for (name, text) in layouts {
        let key = format!("layout:{name}");
        let hash = layout_fingerprint(text);
        let path = dir.join(format!("{name}.json"));
        if path.exists() {
            let current = layout_fingerprint(&std::fs::read_to_string(&path)?);
            let Some(was) = installed.get(&key).and_then(|f| f.get("json")) else {
                // From 0.1.x, which recorded no layouts: whether it was edited is
                // unknowable, so no rescue copy and no toast; kept aside, quietly.
                if current != hash {
                    std::fs::rename(&path, dir.join(format!("{name}.json.before-{}", env!("CARGO_PKG_VERSION"))))?;
                }
                std::fs::write(&path, text)?;
                installed.insert(key, [("json".into(), hash)].into());
                continue;
            };
            if current != *was && current != hash {
                let clone = edited_name(&dir, name, ".json");
                copy_layout_state(home, name, &clone)?;
                std::fs::rename(&path, dir.join(format!("{clone}.json")))?;
                edited_note(&clone);
            }
        }
        std::fs::write(&path, text)?;
        installed.insert(key, [("json".into(), hash)].into());
    }
    write_if_changed(&record, serde_json::to_string_pretty(&installed).unwrap() + "\n")
}

fn layout_fingerprint(text: &str) -> String {
    let canonical = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok().map(|mut v| {
        crate::layout_sync::normalize(&mut v);
        v.sort_all_objects();
        v.to_string()
    });
    fingerprint(canonical.as_deref().unwrap_or(text).as_bytes())
}

pub fn copy_layout_state(home: &Path, name: &str, to: &str) -> std::io::Result<()> {
    let dir = home.join("layouts/.state");
    match std::fs::copy(dir.join(format!("{name}.json")), dir.join(format!("{to}.json"))) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !dir.join(format!("{name}.json")).exists() => {
            match std::fs::remove_file(dir.join(format!("{to}.json"))) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn clone_mod(name: &str, to: &str) -> std::io::Result<()> {
    clone_mod_at(&mods_dir(), name, to)
}

fn clone_mod_at(mods: &Path, name: &str, to: &str) -> std::io::Result<()> {
    if !valid_name(name) || !valid_name(to) {
        return Err(std::io::Error::other("names use letters, digits, - or _"));
    }
    let source = mods.join(name);
    let target = mods.join(to);
    if !source.is_dir() {
        return Err(std::io::Error::other("no such mod"));
    }
    std::fs::create_dir(&target)?;
    fn copy(source: &Path, target: &Path, from: &str, to: &str) -> std::io::Result<()> {
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let dest = target.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                std::fs::create_dir(&dest)?;
                copy(&entry.path(), &dest, from, to)?;
            } else {
                let bytes = std::fs::read(entry.path())?;
                std::fs::copy(entry.path(), &dest)?;
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let mut text = text.replace(&format!("mods/{from}/"), &format!("mods/{to}/"))
                        .replace(&format!("\"mods/{from}\""), &format!("\"mods/{to}\""))
                        .replace(&format!("'mods/{from}'"), &format!("'mods/{to}'"));
                    for key in ["mod", "\"mod\"", "'mod'"] {
                        for space in ["", " "] {
                            for quote in ["\"", "'"] {
                                text = text.replace(&format!("{key}:{space}{quote}{from}{quote}"), &format!("{key}:{space}{quote}{to}{quote}"));
                            }
                        }
                    }
                    // mod.json actions naming the mod's own commands follow it.
                    if entry.file_name() == "mod.json" {
                        text = text.replace(&format!("\"{from}."), &format!("\"{to}."));
                    }
                    std::fs::write(dest, text)?;
                }
            }
        }
        Ok(())
    }
    let result = (|| {
        copy(&source, &target, name, to)?;
        let path = target.join("mod.json");
        let mut meta: Value = if path.exists() { serde_json::from_str(&std::fs::read_to_string(&path)?)? } else { json!({}) };
        let Some(object) = meta.as_object_mut() else {
            return Err(std::io::Error::other("mod.json must be an object"));
        };
        object.insert("cloned_from".into(), json!({ "name": name, "version": env!("CARGO_PKG_VERSION") }));
        std::fs::write(path, serde_json::to_string_pretty(&meta)?)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(target);
    }
    result
}

/// Mods in use by kind, for telemetry: the shipped ones as shipped (by
/// name), how many shipped ones were edited, how many are the person's own
/// (never their names).
pub fn kinds(names: &std::collections::BTreeSet<String>) -> (Vec<String>, usize, usize) {
    let text = std::fs::read_to_string(app_dir().join("installed.json")).unwrap_or_default();
    let installed: Installed = serde_json::from_str(&text).unwrap_or_default();
    let shipped = |n: &str| DEFAULT_MODS.iter().any(|(p, _)| p.split('/').next() == Some(n));
    let (mut builtin, mut edited, mut custom) = (Vec::new(), 0, 0);
    for name in names {
        if !shipped(name) {
            custom += 1;
        } else if installed.get(name).is_some_and(|files| as_shipped(&mods_dir().join(name), files)) {
            builtin.push(name.clone());
        } else {
            edited += 1;
        }
    }
    (builtin, edited, custom)
}

/// Exactly as installed: every file unchanged and none added.
fn as_shipped(dir: &Path, files: &Files) -> bool {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = e.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    // gpui-shell writes these editor helpers when it loads a mod.
    found.retain(|f| !matches!(f.as_str(), "gpui-kit.d.ts" | "jsconfig.json") || files.contains_key(f));
    found.len() == files.len() && found.iter().all(|f| files.contains_key(f)) && untouched(dir, files)
}

/// File path (inside the mod) to content fingerprint.
type Files = std::collections::BTreeMap<String, String>;
/// Default mods Herdr installed, with the files as installed.
type Installed = std::collections::BTreeMap<String, Files>;

/// FNV-1a: stable across builds, unlike std's hasher.
fn fingerprint(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3));
    format!("{h:016x}")
}

/// Whether every file Herdr installed is still there as installed.
fn untouched(dir: &Path, files: &Files) -> bool {
    files.iter().all(|(path, want)| std::fs::read(dir.join(path)).is_ok_and(|b| fingerprint(&b) == *want))
}

fn write_if_changed(path: &Path, content: impl AsRef<[u8]>) -> std::io::Result<()> {
    let content = content.as_ref();
    if std::fs::read(path).ok().as_deref() != Some(content) {
        std::fs::write(path, content)?;
    }
    Ok(())
}

/// Where a mod would like to go the first time it appears (mod.json).
#[derive(Deserialize, Default, Clone)]
pub struct Hint {
    #[serde(default)]
    pub place: Option<String>,
    #[serde(default)]
    pub size: Option<f32>,
    /// `pane.go` can give it the keyboard.
    #[serde(default)]
    pub focusable: bool,
    /// Its command palette entries, as written: `actions` reads them.
    #[serde(default)]
    pub actions: Value,
    /// Its terminal menu entries, as written: `menu_items` reads them.
    #[serde(default)]
    pub menu_items: Value,
}

#[derive(Clone)]
pub struct Found {
    pub id: String,
    pub dir: PathBuf,
    pub hint: Hint,
    /// Why mod.json couldn't be read as one (`hint` is then the default).
    pub hint_error: Option<String>,
    pub stamp: SystemTime,
}

/// Every mod folder that has a main.js.
pub fn scan() -> Vec<Found> {
    let mut found = scan_dir(&mods_dir(), "");
    found.extend(scan_dir(&frame_dir(), "frame-"));
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

fn scan_dir(root: &Path, prefix: &str) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().join("main.js").is_file())
        .filter_map(|e| {
            let id = format!("{prefix}{}", e.file_name().to_str()?);
            if id.starts_with('.') {
                return None;
            }
            let dir = e.path();
            let (hint, hint_error) = read_hint(&dir);
            let stamp = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|f| f.metadata().ok()?.modified().ok())
                .max()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some(Found { id, dir, hint, hint_error, stamp })
        })
        .collect()
}

/// A mod.json that can't be read leaves the defaults, and why.
fn read_hint(dir: &Path) -> (Hint, Option<String>) {
    let Ok(text) = std::fs::read_to_string(dir.join("mod.json")) else { return (Hint::default(), None) };
    match serde_json::from_str(&text) {
        Ok(hint) => (hint, None),
        Err(e) => (Hint::default(), Some(e.to_string())),
    }
}

/// What is wrong with `dir`'s palette actions and terminal menu items, or with
/// a mod.json that costs it them (`herdr-app check`).
pub fn mod_json_problems(dir: &Path) -> Vec<String> {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_owned());
    let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    let (hint, hint_error) = read_hint(&dir);
    let found = Found { id, dir, hint, hint_error, stamp: SystemTime::UNIX_EPOCH };
    let mut problems = actions([&found]).1;
    problems.extend(menu_items([&found]).1);
    problems
}

/// Every mod's palette actions (`modActions()`), in folder order, and what is
/// wrong with the ones left out, or with a mod.json Herdr can't read. An
/// action is `{ title, command, args?, keywords? }` naming an app command;
/// `mod.open` without a `name` opens the mod itself.
pub fn actions<'a>(folders: impl IntoIterator<Item = &'a Found> + Clone) -> (Vec<Value>, Vec<String>) {
    let unreadable = folders.clone().into_iter().filter_map(|f| Some(format!("mod {}: mod.json: {}", f.id, f.hint_error.as_ref()?)));
    let mut problems: Vec<String> = unreadable.collect();
    let (list, more) = entries(folders, "actions", |h| &h.actions, action);
    problems.extend(more);
    (list, problems)
}

/// Every mod's terminal menu items (`menuItems()`), in folder order, and what
/// is wrong with the ones left out. An item is `{ title, command, args?, when?,
/// icon? }`; the menu runs it on the right-clicked terminal (`fill`).
pub fn menu_items<'a>(folders: impl IntoIterator<Item = &'a Found>) -> (Vec<Value>, Vec<String>) {
    entries(folders, "menu_items", |h| &h.menu_items, menu_item)
}

/// One mod.json list, checked entry by entry; a mod.json that couldn't be read
/// is `actions`' to report.
fn entries<'a>(
    folders: impl IntoIterator<Item = &'a Found>,
    field: &str,
    of: fn(&Hint) -> &Value,
    check: fn(&str, &Value) -> Result<Value, String>,
) -> (Vec<Value>, Vec<String>) {
    let mut list = Vec::new();
    let mut problems = Vec::new();
    for f in folders.into_iter().filter(|f| f.hint_error.is_none()) {
        let entries = match of(&f.hint) {
            Value::Null => continue,
            Value::Array(entries) => entries,
            _ => {
                problems.push(format!("mod {}: {field}: expected a list", f.id));
                continue;
            }
        };
        for (i, entry) in entries.iter().enumerate() {
            match check(&f.id, entry) {
                Ok(a) => list.push(a),
                Err(why) => problems.push(format!("mod {}: {field}[{i}]: {why}", f.id)),
            }
        }
    }
    (list, problems)
}

fn action(mod_id: &str, entry: &Value) -> Result<Value, String> {
    let (title, name, args) = match own_command(mod_id, entry) {
        Some(own) => own?,
        None => runnable(mod_id, entry, &[]).map(|(title, name, args, _)| (title, name, args))?,
    };
    let keywords = match entry.get("keywords") {
        None | Some(Value::Null) => "",
        Some(Value::String(k)) => k,
        Some(_) => return Err("keywords: expected a string".into()),
    };
    Ok(json!({ "mod": mod_id, "title": title, "command": name, "args": args, "keywords": keywords }))
}

/// An action naming one of this mod's own commands (`<mod>.<command>`, as
/// keys.json binds them): the palette runs it like a key, delivering it to
/// the mod's `nextCommand()`. None for anything else, an app command too.
fn own_command(mod_id: &str, entry: &Value) -> Option<Result<(String, String, Value), String>> {
    let name = entry.get("command")?.as_str()?;
    let own = name.strip_prefix(mod_id)?.strip_prefix('.')?;
    if own.is_empty() || crate::commands::find(name).is_some() {
        return None;
    }
    let title = entry.get("title").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
    Some(match (title, entry.get("args")) {
        (None, _) => Err("needs a title".into()),
        (Some(title), None | Some(Value::Null)) => Ok((title.to_owned(), name.to_owned(), json!({}))),
        (Some(title), Some(Value::Object(args))) if args.is_empty() => Ok((title.to_owned(), name.to_owned(), json!({}))),
        (Some(_), Some(_)) => Err(format!("{name}: a mod's own command takes no args")),
    })
}

/// When a menu item shows: the terminal had a selection, its folder is known,
/// or it is a tab's pane (not a layout terminal block).
const MENU_WHEN: [&str; 3] = ["selected", "cwd", "pane"];

fn menu_item(mod_id: &str, entry: &Value) -> Result<Value, String> {
    let (title, name, args, mut fill) = runnable(mod_id, entry, &["pane", "terminal"])?;
    let when = match entry.get("when") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(w)) if MENU_WHEN.contains(&w.as_str()) => json!(w),
        Some(_) => return Err(format!("when: expected one of {}", MENU_WHEN.map(|w| format!("{w:?}")).join(", "))),
    };
    let icon = match entry.get("icon") {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(f)) if f.ends_with(".svg") && !f.contains(['/', '\\']) => json!(format!("mods/{mod_id}/{f}")),
        Some(_) => return Err("icon: expected an .svg file in the mod's folder".into()),
    };
    // mod.open's props get the clicked target whole.
    if name == "mod.open" {
        fill.push("props");
    }
    Ok(json!({ "mod": mod_id, "title": title, "command": name, "args": args, "when": when, "icon": icon, "fill": fill }))
}

/// A mod.json entry's title, app command and checked args, and which of
/// `fillable` the command takes but the entry leaves open, for the caller to
/// fill when it runs (so they count as given).
fn runnable(mod_id: &str, entry: &Value, fillable: &[&'static str]) -> Result<(String, String, Value, Vec<&'static str>), String> {
    let text = |k: &str| entry.get(k).and_then(Value::as_str).filter(|s| !s.trim().is_empty());
    let title = text("title").ok_or("needs a title")?;
    let name = text("command").ok_or("needs a command")?;
    let command = crate::commands::find(name).ok_or_else(|| format!("no command {name:?}"))?;
    let mut args = match entry.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(args @ Value::Object(_)) => args.clone(),
        Some(_) => return Err("args: expected an object".into()),
    };
    if name == "mod.open" && crate::commands::missing(command, &json!({ "name": args.get("name") })).is_some() {
        args["name"] = json!(mod_id);
    }
    if name == "mod.open" && !args["name"].is_string() {
        return Err("mod.open: name: expected a mod's folder name".into());
    }
    crate::commands::validate_args(command, &args)?;
    let fill: Vec<&'static str> = fillable
        .iter()
        .copied()
        .filter(|k| command.args.iter().any(|a| a.name == *k) && args.get(*k).is_none_or(Value::is_null))
        .collect();
    let mut given = args.clone();
    for k in &fill {
        given[*k] = json!("filled");
    }
    if let Some(missing) = crate::commands::missing(command, &given) {
        return Err(format!("{name}: needs {missing}"));
    }
    if args.get("props").is_some_and(|p| !p.is_object() && !p.is_null()) {
        return Err(format!("{name}: props: expected an object"));
    }
    Ok((title.to_owned(), name.to_owned(), args, fill))
}

/// What mods ask of the window.
pub enum Request {
    Run { name: String, args: Value },
    OpenOverlay { id: String, spec: Value },
    CloseOverlay { id: String },
    /// The OS window's title; `None` restores the default.
    SetWindowTitle(Option<String>),
    /// Every copy re-renders.
    Refresh,
    /// Copies whose reads changed re-render (`snapshot_changed`).
    Changed,
    /// A look at a folder's git status finished.
    Git(crate::git_status::Update),
    /// From `herdr-app do` / `herdr-app status` (control.rs); answer on `reply`.
    Control { cmd: Value, reply: std::sync::mpsc::Sender<Value> },
    /// `terminalLines`: a terminal's last `lines` lines with their faint stretches, or null.
    Lines { terminal: String, lines: usize, reply: async_channel::Sender<Value> },
}

static REQUESTS: OnceLock<async_channel::Sender<Request>> = OnceLock::new();

thread_local! {
    static FINAL_KILLS: RefCell<Option<(std::sync::Arc<crate::hc::Core>, std::time::Instant)>> = const { RefCell::new(None) };
}

/// The event loop stops before queued commands run at quit. Send final terminal
/// kills to the core now, bounded by the same deadline as the mod's cleanup.
pub fn final_cleanup(core: std::sync::Arc<crate::hc::Core>, deadline: std::time::Instant, body: impl FnOnce()) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) { FINAL_KILLS.with_borrow_mut(|slot| { slot.take(); }); }
    }
    FINAL_KILLS.with_borrow_mut(|slot| {
        assert!(slot.is_none(), "nested final cleanup");
        *slot = Some((core, deadline));
    });
    let _reset = Reset;
    body();
}

/// Navigation order set by the mod that lists agents, with stale entries removed and new agents appended.
pub fn agent_order(available: Vec<String>) -> Vec<String> {
    AGENT_ORDER.with(|s| {
        let mut order = Vec::new();
        for addr in s.borrow().iter().chain(available.iter()) {
            if available.contains(addr) && !order.contains(addr) {
                order.push(addr.clone());
            }
        }
        order
    })
}

pub fn shared(key: &str) -> Option<Value> {
    with_shared(|s| s.values.get(key).cloned())
}

/// Sets a value every mod can read (persisted in shared.json); the copies that read it re-render.
pub fn set_shared(key: String, value: Value) {
    let read_key = shared_key(&key);
    if with_shared(|s| s.set(key, value)) {
        bump_revision(&read_key);
        request(Request::Changed);
    }
}

fn shared_key(key: &str) -> String {
    format!("shared\u{0}{key}")
}

fn with_shared<R>(f: impl FnOnce(&mut SharedStore) -> R) -> R {
    SHARED.with_borrow_mut(|slot| f(slot.get_or_insert_with(|| SharedStore::load(app_dir().join("shared.json")))))
}

/// Called after mod close handlers, so their last changes reach disk before exit.
pub fn finish_shared() {
    SHARED.with_borrow_mut(|slot| {
        if let Some(store) = slot {
            store.finish();
        }
    });
}

struct SharedStore {
    values: serde_json::Map<String, Value>,
    path: PathBuf,
    writable: bool,
    writer: Option<(std::sync::mpsc::Sender<String>, std::thread::JoinHandle<()>)>,
}

impl SharedStore {
    fn load(path: PathBuf) -> Self {
        let mut writable = true;
        let values = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(values) => values,
                Err(error) => {
                    let backup = crate::window::aside(&path, "corrupt");
                    match std::fs::rename(&path, &backup) {
                        Ok(()) => tracing::warn!("{} is invalid ({error}); kept in {}", path.display(), backup.display()),
                        Err(e) => {
                            writable = false;
                            tracing::warn!("{} is invalid ({error}); cannot keep it aside: {e}; shared persistence disabled", path.display());
                        }
                    }
                    serde_json::Map::new()
                }
            },
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    writable = false;
                    tracing::warn!("cannot read {}: {error}; shared persistence disabled", path.display());
                }
                serde_json::Map::new()
            }
        };
        Self { values, path, writable, writer: None }
    }

    fn set(&mut self, key: String, value: Value) -> bool {
        if self.values.get(&key) == Some(&value) {
            return false;
        }
        self.values.insert(key, value);
        if self.writable {
            let (writer, _) = self.writer.get_or_insert_with(|| shared_writer(self.path.clone()));
            if let Err(error) = writer.send(serde_json::to_string(&self.values).expect("shared values encode as JSON")) {
                tracing::warn!("shared writer stopped: {error}");
            }
        }
        true
    }

    fn finish(&mut self) {
        if let Some((sender, worker)) = self.writer.take() {
            drop(sender);
            if worker.join().is_err() {
                tracing::warn!("shared writer stopped before saving finished");
            }
        }
    }
}

fn shared_writer(path: PathBuf) -> (std::sync::mpsc::Sender<String>, std::thread::JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let worker = std::thread::spawn(move || {
        while let Ok(mut text) = rx.recv() {
            // A burst needs only its newest snapshot. Never let an older write land last.
            for newer in rx.try_iter() {
                text = newer;
            }
            if let Err(error) = crate::persist::write_atomic(&path, text) {
                tracing::warn!("cannot write {}: {error}", path.display());
            }
        }
    });
    (tx, worker)
}

pub fn request_sender() -> Option<async_channel::Sender<Request>> {
    REQUESTS.get().cloned()
}

pub fn request(r: Request) {
    if let Some(tx) = REQUESTS.get() {
        let _ = tx.try_send(r);
    }
}

thread_local! {
    /// Snapshots the window publishes; mods read them synchronously and
    /// re-render when they change.
    static SNAPSHOT: RefCell<Snapshot> = RefCell::new(Snapshot::default());
    static AGENT_ORDER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static SHARED: RefCell<Option<SharedStore>> = const { RefCell::new(None) };
    /// Where key commands for each loaded copy go: (copy, mod, sender).
    static COMMANDS: RefCell<Vec<(u64, String, async_channel::Sender<String>)>> = const { RefCell::new(Vec::new()) };
    /// The copies on screen: only they get commands (a hidden copy kept alive must
    /// not act on the person's keys).
    static SHOWN_COPIES: RefCell<std::collections::HashSet<u64>> = RefCell::new(std::collections::HashSet::new());
    static LIVE_READS: RefCell<HashMap<u64, Weak<RefCell<LiveReads>>>> = RefCell::new(HashMap::new());
    /// Each loaded copy's props, which the window can replace without
    /// reloading the copy (`set_props`).
    static LIVE_PROPS: RefCell<HashMap<u64, Weak<RefCell<Value>>>> = RefCell::new(HashMap::new());
    /// How often each thing mods read has changed: a snapshot key, `shared\0<key>`,
    /// a folder's git status (`git_status::Place::read_key`).
    static REVISIONS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    /// `keys()` as last served, with when its files had changed (`keys_part`).
    static KEYS_PART: RefCell<Option<(FileStamp, std::sync::Arc<Value>)>> = const { RefCell::new(None) };
    /// Snapshot parts by key, with the revision they were copied at (`shared_part`).
    static SHARED_PARTS: RefCell<HashMap<&'static str, (u64, std::sync::Arc<Value>)>> = RefCell::new(HashMap::new());
    /// Copies passed over by a refresh while hidden: refreshed once shown.
    static MISSED: RefCell<std::collections::HashSet<u64>> = RefCell::new(std::collections::HashSet::new());
    static COPY_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static OVERLAY_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub struct Snapshot {
    pub layout: Value,
    pub spaces: Value,
    pub machines: Value,
    pub agents: Value,
    /// Every pane: { pane, terminal, tab, title, cwd, branch, status, app, focused, zoomed }.
    pub panes: Value,
    pub terminals: Value,
    pub focused: Value,
    /// The focus history (`focus_history.rs`), newest first.
    pub places: Value,
    /// Floating blocks by id: { edge, align, vertical }.
    pub floats: Value,
    /// { platform, titlebar, buttons, maximized, fullscreen, active }.
    pub window: Value,
    /// The shown layout's spacing fields (`layout_sync::spacing_fields`): a
    /// reader redraws when these change, not on every layout click.
    pub spacing: Value,
    /// Every mod's palette actions (`actions`).
    pub actions: Value,
    /// Every mod's terminal menu items (`menu_items`).
    pub menu_items: Value,
}

impl Default for Snapshot {
    /// Lists start empty, never null, so mods can map over them right away.
    fn default() -> Self {
        Snapshot { layout: Value::Null, spaces: json!([]), machines: json!([]), agents: json!([]), panes: json!([]), terminals: json!([]), focused: Value::Null, places: json!([]), floats: json!({}), window: Value::Null, spacing: Value::Null, actions: json!([]), menu_items: json!([]) }
    }
}

/// Replaces one part of the snapshot; returns whether it changed.
fn publish(key: &'static str, set: impl FnOnce(&mut Snapshot) -> (Value, Value)) -> bool {
    let changed = SNAPSHOT.with(|s| {
        let (old, new) = set(&mut s.borrow_mut());
        old != new
    });
    if changed {
        bump_revision(key);
    }
    changed
}

/// How many times `key` changed, for tests.
#[cfg(test)]
pub fn revision(key: &str) -> u64 {
    REVISIONS.with_borrow(|r| r.get(key).copied().unwrap_or(0))
}

/// Something mods read changed: the copies that read it redraw.
pub fn bump_revision(key: &str) {
    REVISIONS.with_borrow_mut(|r| match r.get_mut(key) {
        Some(n) => *n += 1,
        None => {
            r.insert(key.to_owned(), 1);
        }
    });
}

pub fn publish_layout(layout: &Node) -> bool {
    let v = serde_json::to_value(layout).unwrap_or(Value::Null);
    publish("layout", |s| (std::mem::replace(&mut s.layout, v.clone()), v))
}
pub fn publish_spacing(v: Value) -> bool {
    publish("spacing", |s| (std::mem::replace(&mut s.spacing, v.clone()), v))
}
pub fn publish_machines(v: Value) -> bool {
    publish("machines", |s| (std::mem::replace(&mut s.machines, v.clone()), v))
}
pub fn publish_spaces(v: Value) -> bool {
    publish("spaces", |s| (std::mem::replace(&mut s.spaces, v.clone()), v))
}
pub fn publish_agents(v: Value) -> bool {
    publish("agents", |s| (std::mem::replace(&mut s.agents, v.clone()), v))
}
/// `spaces()` as last published, for tests.
#[cfg(test)]
pub fn published_spaces() -> Value {
    SNAPSHOT.with(|s| s.borrow().spaces.clone())
}
/// `panes()` as last published, for tests.
#[cfg(test)]
pub fn published_panes() -> Value {
    SNAPSHOT.with(|s| s.borrow().panes.clone())
}
/// `agents()` as last published, for tests.
#[cfg(test)]
pub fn published_agents() -> Value {
    SNAPSHOT.with(|s| s.borrow().agents.clone())
}

/// `modActions()` as last published, for tests.
#[cfg(test)]
pub fn published_actions() -> Value {
    SNAPSHOT.with(|s| s.borrow().actions.clone())
}

pub fn publish_panes(v: Value) -> bool {
    publish("panes", |s| (std::mem::replace(&mut s.panes, v.clone()), v))
}
pub fn publish_terminals(mut v: Value) -> bool {
    publish("terminals", |s| {
        let selections: HashMap<_, _> = s.terminals.as_array().into_iter().flatten()
            .filter_map(|t| Some((t["address"].as_str()?, t["selection"].clone()))).collect();
        for terminal in v.as_array_mut().into_iter().flatten() {
            terminal["selection"] = terminal["address"].as_str().and_then(|address| selections.get(address)).cloned().unwrap_or(Value::Null);
        }
        (std::mem::replace(&mut s.terminals, v.clone()), v)
    })
}
/// Only selection changes refresh readers; output and cursor motion do not.
pub fn publish_selection(address: &str, selection: Value) -> bool {
    publish("terminals", |s| {
        let old = s.terminals.clone();
        if let Some(terminal) = s.terminals.as_array_mut().and_then(|list| list.iter_mut().find(|t| t["address"] == address)) {
            terminal["selection"] = selection.clone();
        }
        (old, s.terminals.clone())
    })
}

pub fn publish_focused_selection(selection: Value) -> bool {
    publish("focused", |s| {
        let old = s.focused.clone();
        if s.focused.is_object() {
            s.focused["selection"] = selection;
        }
        (old, s.focused.clone())
    })
}

pub fn terminal_address(word: &str, source: &str) -> Option<String> {
    SNAPSHOT.with(|s| {
        let s = s.borrow();
        let live = s.terminals.as_array()?;
        crate::links::terminal_address(word, source, live.iter().filter_map(|t| t["address"].as_str()))
    })
}

pub fn publish_floats(v: Value) -> bool {
    publish("floats", |s| (std::mem::replace(&mut s.floats, v.clone()), v))
}
pub fn publish_window(v: Value) -> bool {
    publish("window", |s| (std::mem::replace(&mut s.window, v.clone()), v))
}
pub fn publish_focused(mut v: Value) -> bool {
    publish("focused", |s| {
        if v.is_object() {
            v["selection"] = s.focused["selection"].clone();
        }
        (std::mem::replace(&mut s.focused, v.clone()), v)
    })
}
pub fn publish_places(v: Value) -> bool {
    publish("places", |s| (std::mem::replace(&mut s.places, v.clone()), v))
}
pub fn publish_actions(v: Value) -> bool {
    publish("actions", |s| (std::mem::replace(&mut s.actions, v.clone()), v))
}
pub fn publish_menu_items(v: Value) -> bool {
    publish("menu_items", |s| (std::mem::replace(&mut s.menu_items, v.clone()), v))
}

#[derive(Default)]
struct LiveReads(HashMap<String, u64>);

impl LiveReads {
    fn record(&mut self, key: &str, revisions: &HashMap<String, u64>) {
        if !self.0.contains_key(key) {
            self.0.insert(key.to_owned(), revisions.get(key).copied().unwrap_or(0));
        }
    }

    fn changed(&mut self, revisions: &HashMap<String, u64>) -> bool {
        let mut changed = false;
        for (key, seen) in &mut self.0 {
            let revision = revisions.get(key.as_str()).copied().unwrap_or(0);
            changed |= *seen != revision;
            *seen = revision;
        }
        changed
    }
}

fn read(reads: &RefCell<LiveReads>, key: &'static str, f: impl FnOnce(&Snapshot) -> &Value) -> HostValue {
    record(reads, key);
    SNAPSHOT.with(|s| to_host(f(&s.borrow())))
}

fn record(reads: &RefCell<LiveReads>, key: &str) {
    REVISIONS.with_borrow(|r| reads.borrow_mut().record(key, r));
}

fn reader(reads: Rc<RefCell<LiveReads>>, key: &'static str, f: fn(&Snapshot) -> &Value) -> impl Fn(&gpui_shell::HostArguments) -> gpui_shell::HostResult {
    move |_| {
        record(&reads, key);
        Ok(HostValue::Json(shared_part(key, f)))
    }
}

/// `keys()`, listed again only once keys.json or the mods folder changed: a
/// status bar mod reads it every render, and reading the file and looking
/// for every mod a binding names cost more than the rest of its render.
fn keys_part() -> std::sync::Arc<Value> {
    let modified = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let stamp = (modified(&crate::keys::path()), modified(&mods_dir()));
    KEYS_PART.with_borrow_mut(|part| {
        cached_part(part, stamp, || {
            let list: Vec<Value> = crate::keys::listing().into_iter().map(|(s, k, a)| json!({ "section": s, "keys": k, "action": a })).collect();
            Value::Array(list)
        })
    })
}

type FileStamp = (Option<std::time::SystemTime>, Option<std::time::SystemTime>);

fn cached_part(part: &mut Option<(FileStamp, std::sync::Arc<Value>)>, stamp: FileStamp, make: impl FnOnce() -> Value) -> std::sync::Arc<Value> {
    match part {
        Some((seen, value)) if *seen == stamp => value.clone(),
        _ => {
            let value = std::sync::Arc::new(make());
            *part = Some((stamp, value.clone()));
            value
        }
    }
}

/// One snapshot part, copied once per revision and shared by every read of
/// it: the script's copy is built straight from it rather than from a
/// converted copy per call.
fn shared_part(key: &'static str, f: fn(&Snapshot) -> &Value) -> std::sync::Arc<Value> {
    let revision = REVISIONS.with_borrow(|r| r.get(key).copied().unwrap_or(0));
    SHARED_PARTS.with_borrow_mut(|parts| {
        if let Some((seen, part)) = parts.get(key)
            && *seen == revision
        {
            return part.clone();
        }
        let part = std::sync::Arc::new(SNAPSHOT.with(|s| f(&s.borrow()).clone()));
        parts.insert(key, (revision, part.clone()));
        part
    })
}

/// Reads in callbacks and child views count too: keep their dependencies until the copy unloads.
pub fn snapshot_changed(copy: u64) -> bool {
    LIVE_READS.with_borrow(|copies| {
        copies.get(&copy).and_then(Weak::upgrade).is_some_and(|reads| {
            REVISIONS.with_borrow(|r| reads.borrow_mut().changed(r))
        })
    })
}

/// Gives a loaded copy new props, read by its next render (the caller
/// refreshes it). Returns whether the copy is still loaded.
pub fn set_props(copy: u64, props: Value) -> bool {
    LIVE_PROPS.with_borrow(|copies| copies.get(&copy).and_then(Weak::upgrade).map(|p| *p.borrow_mut() = props).is_some())
}

/// Delivers `command` to every shown copy of `mod`. Returns whether any listens.
pub fn send_command(name: &str, command: &str) -> bool {
    let shown = SHOWN_COPIES.with(|s| s.borrow().clone());
    COMMANDS.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|(_, _, tx)| !tx.is_closed());
        let mut any = false;
        for (copy, p, tx) in c.iter() {
            if p == name && shown.contains(copy) {
                any |= tx.try_send(command.to_owned()).is_ok();
            }
        }
        any
    })
}

/// Delivers `command` to one shown copy (the block given the keyboard, not
/// every copy of its mod).
pub fn send_command_to(copy: u64, command: &str) -> bool {
    if !SHOWN_COPIES.with(|s| s.borrow().contains(&copy)) {
        return false;
    }
    COMMANDS.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|(_, _, tx)| !tx.is_closed());
        c.iter().any(|(id, _, tx)| *id == copy && tx.try_send(command.to_owned()).is_ok())
    })
}

/// The copies on screen now (`prepare_load` numbers them); returns those that just came into view.
pub fn set_shown_copies(copies: std::collections::HashSet<u64>) -> Vec<u64> {
    SHOWN_COPIES.with_borrow_mut(|shown| {
        let fresh = copies.difference(shown).copied().collect();
        *shown = copies;
        fresh
    })
}

pub fn is_shown(copy: u64) -> bool {
    SHOWN_COPIES.with_borrow(|s| s.contains(&copy))
}

/// A refresh of every copy passed over hidden `copy`: it redraws once shown.
pub fn missed(copy: u64) {
    MISSED.with_borrow_mut(|m| m.insert(copy));
}

/// Whether `copy`, coming into view, has a refresh to catch up on.
pub fn take_missed(copy: u64) -> bool {
    MISSED.with_borrow_mut(|m| m.remove(&copy))
}

pub fn new_overlay_id() -> String {
    OVERLAY_SEQ.with(|s| {
        s.set(s.get() + 1);
        format!("o{}", s.get())
    })
}

/// One loaded copy of a mod: which folder, with which props.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub name: String,
    pub props: Value,
}

impl Instance {
    pub fn key(&self) -> String {
        if self.props.is_null() { self.name.clone() } else { format!("{}{}", self.name, self.props) }
    }
}

pub fn to_host(v: &Value) -> HostValue {
    match v {
        Value::Null => HostValue::Null,
        Value::Bool(b) => HostValue::Bool(*b),
        Value::Number(n) => HostValue::Number(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => HostValue::Str(s.clone()),
        Value::Array(a) => HostValue::Array(a.iter().map(to_host).collect()),
        Value::Object(o) => HostValue::Object(o.iter().map(|(k, v)| (k.clone(), to_host(v))).collect()),
    }
}

pub fn from_host(v: &HostValue) -> Value {
    match v {
        HostValue::Null => Value::Null,
        HostValue::Bool(b) => Value::Bool(*b),
        HostValue::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => Value::from(*n as i64),
        HostValue::Number(n) => serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number),
        HostValue::Str(s) => Value::String(s.clone()),
        HostValue::Array(a) => Value::Array(a.iter().map(from_host).collect()),
        HostValue::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), from_host(v))).collect()),
        HostValue::Json(v) => (**v).clone(),
    }
}

pub const DECLARATIONS: &str = include_str!("../defaults/herdr.d.ts");

/// `(folder, { on })`: the folder as given, without a trailing slash.
fn git_place(a: &gpui_shell::HostArguments) -> Result<crate::git_status::Place, HostError> {
    let folder = a.string(0)?;
    let folder = if folder.len() > 1 { folder.trim_end_matches('/') } else { folder };
    if folder.is_empty() {
        return Err(HostError::new("gitStatus needs a folder"));
    }
    Ok(crate::git_status::Place { on: opt_str(a.get(1), "on").filter(|m| !m.is_empty()), folder: folder.to_owned() })
}

fn strings(v: Option<&HostValue>) -> Vec<String> {
    v.and_then(HostValue::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

fn opt_str(v: Option<&HostValue>, key: &str) -> Option<String> {
    v.and_then(|o| o.get(key)).and_then(HostValue::as_str).map(str::to_owned)
}

const EXEC_LIMIT: Duration = Duration::from_secs(30);
const EXEC_MAX: Duration = Duration::from_secs(600);
const EXEC_OUTPUT: u64 = 8 << 20;

fn exec_limit(options: Option<&HostValue>) -> Result<Duration, HostError> {
    match options.and_then(|o| o.get("timeout")) {
        // `timeout: undefined` reaches here as Null: both mean the default, as omitting it does.
        None | Some(HostValue::Null) => Ok(EXEC_LIMIT),
        Some(HostValue::Number(ms)) if ms.is_finite() && *ms > 0.0 => Ok(Duration::from_secs_f64(ms.min(EXEC_MAX.as_millis() as f64) / 1000.0)),
        _ => Err(HostError::new("timeout must be a positive number of milliseconds")),
    }
}

pub(crate) fn run_blocking(command: &str, args: &[String], cwd: Option<String>, limit: Duration) -> Result<(i32, String, String), String> {
    use std::io::Read;
    // The core may have no `hc` link beside it (a Windows zip has only herdr-core.exe).
    let program = if command == "hc" { crate::core_bin::path() } else { PathBuf::from(command) };
    let mut child = crate::spawn::command(program)
        .args(args)
        .current_dir(cwd.map(PathBuf::from).unwrap_or_else(home))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {command}: {e}"))?;
    // Bounded: a chatty command can't fill memory; the rest is drained and dropped.
    let drain = |r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut r = r;
            let _ = (&mut r).take(EXEC_OUTPUT).read_to_end(&mut buf);
            let _ = std::io::copy(&mut r, &mut std::io::sink());
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = drain(Box::new(child.stdout.take().unwrap()));
    let err = drain(Box::new(child.stderr.take().unwrap()));
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{command} took longer than {} s and was stopped", limit.as_secs_f64()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok((status.code().unwrap_or(-1), out.join().unwrap_or_default(), err.join().unwrap_or_default()))
}

/// Runs a program on another machine through the core's exec: its exact output.
pub fn run_remote(machine: &str, command: &str, args: &[String], cwd: Option<String>) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    run_remote_with_limit(machine, command, args, cwd, EXEC_LIMIT)
}

fn run_remote_with_limit(machine: &str, command: &str, args: &[String], cwd: Option<String>, limit: Duration) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    use base64::Engine;
    let mut hc_args: Vec<String> = vec!["exec".into(), "--json".into(), "--on".into(), machine.into(), "--timeout".into(), limit.as_secs_f64().to_string()];
    if let Some(c) = cwd {
        hc_args.extend(["--cwd".into(), c]);
    }
    hc_args.push("--".into());
    hc_args.push(command.into());
    hc_args.extend(args.iter().cloned());
    // Let the remote timeout end the program and deliver its result before the wrapper expires.
    let (_, out, err) = run_blocking("hc", &hc_args, None, limit + Duration::from_secs(5))?;
    let v: Value = serde_json::from_str(out.trim()).or_else(|_| serde_json::from_str(err.trim())).map_err(|_| format!("hc exec: {}", err.trim()))?;
    if let Some(e) = v.get("error") {
        return Err(e.get("message").and_then(Value::as_str).unwrap_or("hc exec failed").to_owned());
    }
    if v.get("timed_out").and_then(Value::as_bool) == Some(true) {
        return Err(format!("{command} took longer than {} s on {machine} and was stopped", limit.as_secs_f64()));
    }
    let bytes = |k: &str| base64::engine::general_purpose::STANDARD.decode(v.get(k).and_then(Value::as_str).unwrap_or_default()).map_err(|e| e.to_string());
    let code = v.get("code").and_then(Value::as_i64).map_or(-1, |c| c as i32);
    Ok((code, bytes("stdout")?, bytes("stderr")?))
}

/// The agents a machine can start (`knownAgents()`), asked at most once a minute.
fn known_agents(on: Option<String>, list: Vec<Value>) -> Result<Value, String> {
    type InstalledCache = std::collections::HashMap<Option<String>, (std::time::Instant, Vec<String>, Vec<String>)>;
    static SEEN: std::sync::Mutex<Option<InstalledCache>> = std::sync::Mutex::new(None);
    let commands: Vec<String> = list.iter().filter_map(|a| a["run"][0].as_str().map(str::to_owned)).collect();
    let cached = SEEN.lock().ok().and_then(|s| s.as_ref()?.get(&on).filter(|(at, checked, _)| at.elapsed().as_secs() < 60 && checked == &commands).map(|(_, _, v)| v.clone()));
    let have = if let Some(cached) = cached { cached } else {
        let found = crate::programs::installed(&commands, on.as_deref())?;
        if let Ok(mut s) = SEEN.lock() {
            s.get_or_insert_with(std::collections::HashMap::new).insert(on.clone(), (std::time::Instant::now(), commands, found.clone()));
        }
        found
    };
    Ok(Value::Array(
        list.into_iter()
            .map(|mut a| {
                let run = a["run"][0].as_str().unwrap_or_default().to_owned();
                a["installed"] = json!(have.contains(&run));
                a
            })
            .collect(),
    ))
}

/// A text file on another machine: `cat`, or `type` where there is none (Windows).
fn read_remote(machine: &str, path: &str) -> Result<Option<String>, String> {
    let (code, out, _) = match run_remote(machine, "cat", &["--".into(), path.into()], None) {
        Err(e) if e.contains("cannot start") => run_remote(machine, "cmd", &["/d".into(), "/c".into(), "type".into(), path.replace('/', "\\")], None)?,
        r => r?,
    };
    if code != 0 {
        return Ok(None);
    }
    if out.len() as u64 > crate::mod_files::LIMIT {
        return Err("Text file exceeds 1 MiB".into());
    }
    Ok(Some(String::from_utf8_lossy(&out).into_owned()))
}

/// Blocking work for a mod's async host function, off the window thread.
/// Under test it runs when first polled instead: a thread of its own would
/// wake GPUI's test scheduler from outside, which fails the test as not
/// deterministic.
pub(crate) fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = Option<T>> + Send {
    #[cfg(test)]
    {
        async move { Some(work()) }
    }
    #[cfg(not(test))]
    {
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(work());
        });
        async move { rx.recv().await.ok() }
    }
}

fn exec_future(command: String, args: Vec<String>, cwd: Option<String>, on: Option<String>, limit: Duration) -> impl std::future::Future<Output = Result<HostValue, HostError>> + Send {
    let ran = off_thread(move || match on {
        Some(m) => run_remote_with_limit(&m, &command, &args, cwd, limit).map(|(c, o, e)| (c, String::from_utf8_lossy(&o).into_owned(), String::from_utf8_lossy(&e).into_owned())),
        None => run_blocking(&command, &args, cwd, limit),
    });
    async move {
        let (code, stdout, stderr) = ran.await.ok_or_else(|| HostError::new("the command was lost"))?.map_err(HostError::new)?;
        Ok(HostObject::new().field("code", code as f64).field("stdout", stdout).field("stderr", stderr).into())
    }
}

fn module(inst: Instance, copy: u64) -> HostModule {
    let reads = Rc::new(RefCell::new(LiveReads::default()));
    LIVE_READS.with_borrow_mut(|copies| {
        copies.retain(|_, reads| reads.strong_count() > 0);
        copies.insert(copy, Rc::downgrade(&reads));
    });
    let props = inst.props.clone();
    let live_props = Rc::new(RefCell::new(inst.props.clone()));
    LIVE_PROPS.with_borrow_mut(|copies| {
        copies.retain(|_, props| props.strong_count() > 0);
        copies.insert(copy, Rc::downgrade(&live_props));
    });
    let feedback_frame = inst.name == "frame-feedback";
    let feedback_from = inst.props.get("from").and_then(Value::as_str).unwrap_or("person").to_owned();
    let feedback_layout = inst.props.get("layout").and_then(Value::as_str).unwrap_or("other").to_owned();
    let (cmd_tx, commands) = async_channel::unbounded::<String>();
    COMMANDS.with_borrow_mut(|commands| {
        commands.retain(|(_, _, tx)| !tx.is_closed());
        commands.push((copy, inst.name.clone(), cmd_tx));
    });
    HostModule::new("herdr")
        .declarations(DECLARATIONS)
        .async_function("exec", |a| {
            let command = a.string(0)?.to_owned();
            Ok(exec_future(command, strings(a.get(1)), opt_str(a.get(2), "cwd"), opt_str(a.get(2), "on"), exec_limit(a.get(2))?))
        })
        .async_function("readTextFile", |a| {
            let path = a.string(0)?.to_owned();
            let on = opt_str(a.get(1), "on");
            Ok(crate::mod_files::read_with(move || match on {
                Some(m) => read_remote(&m, &path),
                None => crate::mod_files::read(Path::new(&path)),
            }))
        })
        .async_function("writeTextFile", |a| {
            let expected = a.get(2).and_then(|v| match v { HostValue::Str(s) => Some(s.clone()), _ => None });
            Ok(crate::mod_files::write_text(a.string(0)?.to_owned(), a.string(1)?.to_owned(), expected))
        })
        .async_function("hc", |a| Ok(exec_future("hc".into(), strings(a.get(0)), None, None, EXEC_LIMIT)))
        .async_function("terminalLines", |a| {
            let terminal = a.string(0)?.to_owned();
            let lines = a.get(1).and_then(|o| o.get("lines")).and_then(HostValue::as_number).map_or(usize::MAX, |n| n.max(0.0) as usize);
            let (reply, answer) = async_channel::bounded(1);
            request(Request::Lines { terminal, lines, reply });
            Ok(async move { answer.recv().await.map(|v| to_host(&v)).map_err(|_| HostError::new("no window to read terminals from")) })
        })
        .function("home", |_| Ok(home().to_string_lossy().into_owned().into()))
        .function("appHome", |_| Ok(app_dir().to_string_lossy().into_owned().into()))
        .function("fonts", |_| Ok(to_host(&json!(*FONTS.lock().unwrap()))))
        .function("updates", |_| Ok(to_host(&crate::update::status())))
        .function("presentUpdateNotes", |a| {
            crate::update_notes::presented(&strings(a.get(0)));
            Ok(HostValue::Null)
        })
        .function("telemetry", |_| Ok(to_host(&crate::telemetry::status())))
        .function("feedbackPreview", move |a| {
            let images = a.get(2).and_then(|v| match v { HostValue::Number(n) => Some(*n as usize), _ => None }).unwrap_or(0);
            let body = crate::feedback::draft(a.string(0)?, a.string(1)?, images, &feedback_from, &feedback_layout);
            Ok(to_host(&json!({ "body": body, "available": crate::feedback::available() })))
        })
        .function("feedbackPasteImage", move |_| {
            if !feedback_frame { return Err(HostError::new("feedback screenshots are frame-only")); }
            let bytes = gpui_shell::with_current_app(|cx| {
                cx.read_from_clipboard()?.entries().iter().find_map(|e| match e {
                    gpui::ClipboardEntry::Image(i) => Some(i.bytes.clone()),
                    _ => None,
                })
            }).flatten();
            Ok(match bytes {
                None => HostValue::Null,
                Some(b) => to_host(&match crate::feedback::attach(&b) { Ok(path) => json!({ "path": path }), Err(e) => json!({ "error": e }) }),
            })
        })
        .function("feedbackAttachImages", move |a| {
            if !feedback_frame { return Err(HostError::new("feedback screenshots are frame-only")); }
            let paths = strings(a.get(0)).into_iter().map(std::path::PathBuf::from).collect::<Vec<_>>();
            Ok(to_host(&json!(crate::feedback::attach_files(&paths))))
        })
        .async_function("feedbackPickImages", move |_| {
            if !feedback_frame { return Err(HostError::new("feedback screenshots are frame-only")); }
            let picked = gpui_shell::with_current_app(|cx| cx.prompt_for_paths(gpui::PathPromptOptions {
                files: true, directories: false, multiple: true, prompt: Some("Add".into()),
            })).ok_or_else(|| HostError::new("no window to pick from"))?;
            Ok(async move {
                let paths = picked.await.ok().and_then(Result::ok).flatten().unwrap_or_default();
                let out = crate::feedback::attach_files(&paths);
                Ok(to_host(&Value::Array(out)))
            })
        })
        .async_function("knownAgents", |a| {
            let on = opt_str(a.get(0), "on");
            // The table is loaded on the window thread; workers have only defaults.
            let list = crate::programs::startable();
            let found = off_thread(move || known_agents(on, list));
            Ok(async move {
                let agents = found.await.ok_or_else(|| HostError::new("the agent check was lost"))?.map_err(HostError::new)?;
                Ok(to_host(&agents))
            })
        })
        .async_function("pickFolder", |a| {
            let prompt = a.get(0).map(from_host).and_then(|v| v.get("prompt").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| "Open".into());
            let picked = gpui_shell::with_current_app(|cx| cx.prompt_for_paths(gpui::PathPromptOptions {
                files: false, directories: true, multiple: false, prompt: Some(prompt.into()),
            })).ok_or_else(|| HostError::new("no window to pick from"))?;
            Ok(async move {
                let path = picked.await.ok().and_then(Result::ok).flatten().and_then(|p| p.into_iter().next());
                Ok(path.map(|p| HostValue::Str(p.to_string_lossy().into_owned())).unwrap_or(HostValue::Null))
            })
        })
        .function("feedbackClearImages", move |_| {
            if feedback_frame { crate::feedback::clear_shots(); }
            Ok(HostValue::Null)
        })
        .async_function("sendFeedback", move |a| {
            if !feedback_frame { return Err(HostError::new("sending feedback is frame-only")); }
            let body = from_host(a.value(0)?);
            let shots: Vec<String> = from_host(a.value(1).unwrap_or(&HostValue::Null)).as_array()
                .map(|l| l.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            Ok(async move {
                Ok(match crate::feedback::send(body, shots).await {
                    Ok(()) => HostValue::Null,
                    Err(why) => HostValue::Str(why),
                })
            })
        })
        .function("themes", |_| {
            let list: Vec<Value> = crate::settings::themes().into_iter().map(|(name, colors)| json!({ "light": crate::settings::theme_is_light(&name), "name": name, "colors": colors })).collect();
            Ok(to_host(&Value::Array(list)))
        })
        .function("playSound", |a| {
            crate::sound::play(a.string(0)?);
            Ok(HostValue::Null)
        })
        .function("run", |a| {
            let name = a.string(0)?.to_owned();
            let args = a.get(1).map(from_host).unwrap_or(Value::Null);
            if let Some(command) = crate::commands::find(&name) {
                crate::commands::validate_args(command, &args).map_err(HostError::new)?;
            }
            let final_kill = (name == "terminal.kill").then(|| FINAL_KILLS.with_borrow(Clone::clone)).flatten();
            if let Some((core, deadline)) = final_kill {
                if let Err(error) = core.call_timeout(&name, args, deadline.saturating_duration_since(std::time::Instant::now())) {
                    tracing::warn!("final terminal.kill: {error:#}");
                }
            } else {
                request(Request::Run { name, args });
            }
            Ok(HostValue::Null)
        })
        .function("spaces", reader(reads.clone(), "spaces", |s| &s.spaces))
        .function("machines", reader(reads.clone(), "machines", |s| &s.machines))
        .function("agents", reader(reads.clone(), "agents", |s| &s.agents))
        .function("panes", reader(reads.clone(), "panes", |s| &s.panes))
        .function("setAgentOrder", |a| {
            AGENT_ORDER.with(|s| *s.borrow_mut() = strings(a.get(0)));
            Ok(HostValue::Null)
        })
        .function("setWindowTitle", |a| {
            let title = match a.get(0) {
                Some(v) if !v.is_null() => Some(a.string(0)?.to_owned()),
                _ => None,
            };
            request(Request::SetWindowTitle(title));
            Ok(HostValue::Null)
        })
        .function("terminals", reader(reads.clone(), "terminals", |s| &s.terminals))
        .function("focused", reader(reads.clone(), "focused", |s| &s.focused))
        .function("recentPlaces", reader(reads.clone(), "places", |s| &s.places))
        .function("layout", reader(reads.clone(), "layout", |s| &s.layout))
        .function("layoutSpacing", reader(reads.clone(), "spacing", |s| &s.spacing))
        .function("layouts", |_| Ok(to_host(&json!(crate::window::layout_names()))))
        .function("modActions", reader(reads.clone(), "actions", |s| &s.actions))
        .function("menuItems", reader(reads.clone(), "menu_items", |s| &s.menu_items))
        .function("commands", |_| {
            let list: Vec<Value> = crate::commands::COMMANDS
                .iter()
                .map(|c| {
                    let args: Vec<Value> = c.args.iter().map(|a| json!({ "name": a.name, "required": a.required })).collect();
                    json!({ "name": c.name, "args": args, "about": c.about })
                })
                .collect();
            Ok(to_host(&Value::Array(list)))
        })
        .function("window", reader(reads.clone(), "window", |s| &s.window))
        .function("theme", {
            let reads = reads.clone();
            move |_| {
                record(&reads, "theme");
                Ok(to_host(&serde_json::to_value(&*crate::settings::theme()).unwrap_or(Value::Null)))
            }
        })
        .function("settings", |_| Ok(HostValue::Json(crate::settings::for_mods())))
        .function("toastLooks", |_| Ok(HostValue::Json(crate::settings::toast_looks())))
        .function("keys", |_| Ok(HostValue::Json(keys_part())))
        .function("openOverlay", |a| {
            let mut spec = from_host(a.value(0)?);
            if spec.get("mod").is_none()
                && let Some(old) = spec.get("piece").cloned()
            {
                spec["mod"] = old;
            }
            if spec.get("mod").and_then(Value::as_str).is_none() && !spec.get("layout").is_some_and(Value::is_object) {
                return Err(HostError::new("openOverlay needs { mod: \"<folder>\" } or { layout: {...} }"));
            }
            let id = new_overlay_id();
            request(Request::OpenOverlay { id: id.clone(), spec });
            Ok(id.into())
        })
        .function("closeOverlay", |a| {
            request(Request::CloseOverlay { id: a.string(0)?.to_owned() });
            Ok(HostValue::Null)
        })
        .function("notify", |a| {
            let id = new_overlay_id();
            let n = from_host(a.value(0)?);
            request(Request::OpenOverlay { id: id.clone(), spec: json!({ "kind": "toast", "notification": n }) });
            Ok(id.into())
        })
        .function("placement", {
            let float = props.get("float").and_then(Value::as_str).map(str::to_owned);
            let reads = reads.clone();
            move |_| Ok(float.as_ref().map_or(HostValue::Null, |id| read(&reads, "floats", |s| s.floats.get(id).unwrap_or(&Value::Null))))
        })
        .function("props", move |_| Ok(to_host(&live_props.borrow())))
        .function("shared", {
            let reads = reads.clone();
            move |a| {
                let k = a.string(0)?;
                record(&reads, &shared_key(k));
                Ok(with_shared(|s| s.values.get(k).map(to_host).unwrap_or(HostValue::Null)))
            }
        })
        .function("setShared", |a| {
            set_shared(a.string(0)?.to_owned(), from_host(a.value(1)?));
            Ok(HostValue::Null)
        })
        .cancellable_async_function("watchFolder", HostValue::Array(vec![]), |a| {
            let path = a.string(0)?.to_owned();
            Ok(async move { crate::files::next_change(path).await.map(|v| to_host(&json!(v))).map_err(HostError::new) })
        })
        .function("gitStatus", {
            let reads = reads.clone();
            move |a| {
                let place = git_place(a)?;
                let files = a.get(1).and_then(|o| o.get("files")).and_then(HostValue::as_bool).unwrap_or(false);
                record(&reads, &place.read_key());
                Ok(crate::git_status::read(&place, |v, revision, _| to_host(&crate::git_status::shown(v, revision, files))))
            }
        })
        .function("fitHeaderInfo", |a| {
            crate::header_info::fit_for_mods(from_host(a.value(0)?), a.number(1)? as f32, a.number(2)? as f32).map(|v| to_host(&v)).map_err(HostError::new)
        })
        .function("headerInfoPreview", |a| Ok(to_host(&crate::header_info::preview_for_mods(a.string(0)?))))
        .function("cardInfoPreview", |a| Ok(to_host(&crate::header_info::card_preview_for_mods(a.get(0).and_then(HostValue::as_str)))))
        .function("headerTextWidth", |a| Ok(HostValue::Number(crate::header_info::width(a.string(0)?, a.number(1)? as f32) as f64)))
        .async_function("nextGitStatus", |a| {
            let place = git_place(a)?;
            let options = a.get(1);
            let files = options.and_then(|o| o.get("files")).and_then(HostValue::as_bool).unwrap_or(false);
            let after = options.and_then(|o| o.get("after")).and_then(HostValue::as_number).map(|n| n as u64);
            let next = crate::git_status::next(&place, after, files);
            Ok(async move { next.recv().await.map(|v| to_host(&v)).map_err(|_| HostError::new("herdr-app is shutting down")) })
        })
        .function("refreshGitStatus", |a| {
            crate::git_status::refresh(&git_place(a)?);
            Ok(HostValue::Null)
        })
        .async_function("nextCommand", move |_| {
            let rx = commands.clone();
            Ok(async move { rx.recv().await.map(HostValue::Str).map_err(|_| HostError::new("herdr-app is shutting down")) })
        })
}

/// The runtime's base layer, themed from settings.json.
pub fn init(cx: &mut App) {
    gpui_shell::init(cx);
    apply_theme(cx);
}

/// Maps the theme onto the tokens mods read through `cx.theme().colors`.
pub fn apply_theme(cx: &mut App) {
    let t = crate::settings::theme();
    let g = |k: &str| t.get(k).cloned().unwrap_or_default();
    let palette = json!({
        "background": g("background"), "foreground": g("foreground"),
        "surface": g("surface"), "surface_foreground": g("foreground"),
        "primary": g("accent"), "primary_foreground": g("accent_foreground"),
        "secondary": g("raised"), "secondary_foreground": g("foreground"),
        "muted": g("raised"), "muted_foreground": g("muted"),
        "accent": g("hover"), "accent_foreground": g("foreground"),
        "destructive": g("blocked"), "destructive_foreground": g("background"),
        "border": g("border"), "input": g("input"), "ring": g("ring"),
        "selection": g("selection"),
    });
    match serde_json::from_value::<gpui_base::ColorTokens>(palette) {
        Ok(colors) => {
            let theme = gpui_base::Theme::global_mut(cx);
            theme.appearance = if crate::settings::get().dark { gpui_base::ThemeAppearance::Dark } else { gpui_base::ThemeAppearance::Light };
            theme.tokens.colors = colors;
        }
        Err(e) => tracing::error!("theme did not map onto mod tokens: {e}"),
    }
}

fn capabilities() -> Capabilities {
    Capabilities::new()
        .read_roots([home()])
        .write_roots([app_dir()])
        .execute(ExecuteGrant::Unrestricted)
        .network(NetworkGrant::Unrestricted)
        .storage(true)
        .clipboard_read(true)
        .clipboard_write(true)
}

/// Checks exercise shared state in memory, without reading or writing the person's file.
pub fn install_check_policy() {
    SHARED.with_borrow_mut(|slot| *slot = Some(SharedStore { values: Default::default(), path: app_dir().join("shared.json"), writable: false, writer: None }));
    install_policy();
}

/// What mods may touch, and the `herdr` module they import.
pub fn install_policy() -> async_channel::Receiver<Request> {
    gpui_shell::policy::update_default(|p| {
        p.with_capabilities(capabilities())
            .with_host_module(module(Instance { name: String::new(), props: Value::Null }, 0))
            .expect("herdr module is valid")
    });
    let (tx, rx) = async_channel::unbounded();
    let _ = REQUESTS.set(tx);
    rx
}

/// Gives the next mod copy to load a policy of its own: its `herdr` module
/// (its props, its identity) and its localStorage file. The shell hands every
/// view a copy of the default that *shares* modules and storage, so each copy
/// needs a fresh default rather than an edit of the old one.
/// Readies the next load of `inst`; returns the copy's number (for commands).
pub fn prepare_load(inst: &Instance) -> u64 {
    let copy = COPY_SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let dir = app_dir().join("data").join(&inst.name);
    let _ = std::fs::create_dir_all(&dir);
    let file = if inst.props.is_null() {
        "store.json".to_owned()
    } else {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        inst.props.to_string().hash(&mut h);
        format!("store-{:016x}.json", h.finish())
    };
    let policy = gpui_shell::policy::Policy::new()
        .with_application(inst.key())
        .with_capabilities(capabilities())
        .with_storage_path(dir.join(file))
        .with_host_module(module(inst.clone(), copy))
        .expect("herdr module is valid");
    gpui_shell::policy::set_default(policy);
    copy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_input_selection_uses_the_theme_selection_colour() {
        let mut tokens = gpui_base::SemanticThemeTokens::default();
        tokens.colors.accent = gpui::hsla(0.1, 0.5, 0.5, 1.0);
        tokens.colors.selection = gpui::hsla(0.6, 0.5, 0.5, 0.3);

        let style = gpui_base::input::InputEditorStyle::default().resolved(&tokens);

        assert_eq!(style.selection, tokens.colors.selection);
    }

    #[test]
    fn exec_timeout_defaults_caps_and_rejects_invalid_values() {
        assert_eq!(exec_limit(None).unwrap(), EXEC_LIMIT);
        assert_eq!(exec_limit(Some(&HostObject::new().field("timeout", HostValue::Null).into())).unwrap(), EXEC_LIMIT);
        let options = |ms| HostObject::new().field("timeout", ms).into();
        assert_eq!(exec_limit(Some(&options(1250.0))).unwrap(), Duration::from_millis(1250));
        assert_eq!(exec_limit(Some(&options(900_000.0))).unwrap(), EXEC_MAX);
        for ms in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(exec_limit(Some(&options(ms))).is_err());
        }
        assert!(exec_limit(Some(&HostObject::new().field("timeout", "30").into())).is_err());
    }

    #[test]
    fn hc_runs_the_core_the_app_uses_even_without_an_hc_link() {
        const CHILD: &str = "HERDR_TEST_HC_IS_THE_CORE";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "mods::tests::hc_runs_the_core_the_app_uses_even_without_an_hc_link", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            return;
        }
        let dir = std::env::temp_dir().join(format!("herdr-hc-is-the-core-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        let core = {
            use std::os::unix::fs::PermissionsExt;
            let core = dir.join("herdr-core");
            std::fs::write(&core, "#!/bin/sh\necho \"the chosen core: $*\"\n").unwrap();
            std::fs::set_permissions(&core, std::fs::Permissions::from_mode(0o755)).unwrap();
            core
        };
        #[cfg(windows)]
        let core = {
            let core = dir.join("herdr-core.cmd");
            std::fs::write(&core, "@echo the chosen core: %*\r\n").unwrap();
            core
        };
        crate::core_bin::use_core(core);
        let (code, stdout, stderr) = run_blocking("hc", &["ls".into()], None, Duration::from_secs(5)).unwrap();
        assert_eq!((code, stdout.trim()), (0, "the chosen core: ls"), "{stderr}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn exec_uses_the_requested_deadline_and_reports_it() {
        let args = ["-c".into(), "sleep 0.15; printf done".into()];
        let error = run_blocking("sh", &args, None, Duration::from_millis(30)).unwrap_err();
        assert!(error.contains("0.03 s"), "{error}");
        let (code, stdout, _) = run_blocking("sh", &args, None, Duration::from_secs(2)).unwrap();
        assert_eq!((code, stdout.as_str()), (0, "done"));
    }

    #[cfg(unix)]
    #[gpui::test]
    async fn exec_returns_real_output_and_errors_under_the_test_scheduler(_cx: &mut gpui::TestAppContext) {
        let result = exec_future("sh".into(), vec!["-c".into(), "printf out; printf err >&2; exit 7".into()], None, None, EXEC_LIMIT).await.unwrap();
        assert!(matches!(result.get("code"), Some(HostValue::Number(7.0))));
        assert!(matches!(result.get("stdout"), Some(HostValue::Str(s)) if s == "out"));
        assert!(matches!(result.get("stderr"), Some(HostValue::Str(s)) if s == "err"));
        assert!(exec_future("herdr_test_missing_command_638".into(), vec![], None, None, EXEC_LIMIT).await.is_err());
        let error = exec_future("sh".into(), vec!["-c".into(), "sleep 0.15; printf late".into()], None, None, Duration::from_millis(30)).await.unwrap_err();
        assert!(error.to_string().contains("0.03 s"), "{error}");
    }

    #[test]
    fn live_reads_ignore_unread_inputs_and_keep_pending_changes() {
        let mut revisions = HashMap::new();
        let mut list = LiveReads::default();
        let mut chrome = LiveReads::default();
        list.record("agents", &revisions);
        chrome.record("layout", &revisions);
        revisions.insert("terminals".into(), 1);
        assert!(!list.changed(&revisions));
        assert!(!chrome.changed(&revisions));
        revisions.insert("agents".into(), 1);
        // A callback reading the new snapshot must not swallow a pending redraw.
        list.record("agents", &revisions);
        assert!(list.changed(&revisions));
        assert!(!chrome.changed(&revisions));
        assert!(!list.changed(&revisions));
        // Dependencies first read by a child or a callback remain subscribed.
        list.record("focused", &revisions);
        revisions.insert("focused".into(), 1);
        assert!(list.changed(&revisions));
    }

    #[test]
    fn selection_snapshots_survive_core_refresh() {
        std::thread::spawn(|| {
            let list = json!([{"address":"t1"}, {"address":"t2"}]);
            publish_terminals(list.clone());
            let selection = json!({"start":{"row":5,"col":2},"end":{"row":1,"col":8}});
            publish_selection("t1", selection.clone());
            publish_terminals(list);
            SNAPSHOT.with_borrow(|s| {
                assert_eq!(s.terminals[0]["selection"], selection);
                assert_eq!(s.terminals[1]["selection"], Value::Null);
            });
            publish_selection("t1", Value::Null);
            SNAPSHOT.with_borrow(|s| assert_eq!(s.terminals[0]["selection"], Value::Null));
        }).join().unwrap();
    }

    #[test]
    fn equal_snapshots_do_not_refresh_readers() {
        std::thread::spawn(|| {
            let reads = Rc::new(RefCell::new(LiveReads::default()));
            LIVE_READS.with_borrow_mut(|copies| { copies.insert(1, Rc::downgrade(&reads)); });
            read(&reads, "agents", |s| &s.agents);
            assert!(!snapshot_changed(1));
            assert!(!publish_agents(json!([])));
            assert!(!snapshot_changed(1));
            assert!(publish_terminals(json!([{"address":"t1"}])));
            assert!(!snapshot_changed(1));
            assert!(publish_agents(json!([{"address":"t1","status":"working"}])));
            assert!(snapshot_changed(1));
            assert!(!snapshot_changed(1));
            assert!(!publish_agents(json!([{"address":"t1","status":"working"}])));
            assert!(!snapshot_changed(1));
            drop(reads);
            assert!(!snapshot_changed(1));
        }).join().unwrap();
    }

    /// A focus change that shades the sidebar bumps `theme`: only the copies
    /// whose `herdr` module answered `theme()` redraw.
    #[test]
    fn a_theme_bump_refreshes_only_theme_readers() {
        std::thread::spawn(|| {
            let none = gpui_shell::HostArguments::new([]);
            let themed = module(Instance { name: "themed".into(), props: Value::Null }, 101);
            let other = module(Instance { name: "other".into(), props: Value::Null }, 102);
            themed.call("theme", &none).unwrap();
            other.call("spaces", &none).unwrap();
            assert!(!snapshot_changed(101) && !snapshot_changed(102));
            bump_revision("theme");
            assert!(snapshot_changed(101), "a copy that read theme() redraws");
            assert!(!snapshot_changed(102), "one that didn't stays");
        }).join().unwrap();
    }

    /// `settings()` hands out the value built at the last settings change,
    /// without building it again per call.
    #[test]
    fn settings_hands_out_the_value_built_at_the_last_change() {
        std::thread::spawn(|| {
            let host = module(Instance { name: "reader".into(), props: Value::Null }, 103);
            let none = gpui_shell::HostArguments::new([]);
            match host.call("settings", &none).unwrap() {
                HostValue::Json(v) => assert!(std::sync::Arc::ptr_eq(&v, &crate::settings::for_mods())),
                other => panic!("settings() answered {other:?}"),
            }
        }).join().unwrap();
    }

    #[test]
    fn keys_are_listed_again_only_when_their_files_change() {
        let at = |s| (Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(s)), None);
        let mut part = None;
        assert_eq!(*cached_part(&mut part, at(1), || json!("one")), json!("one"));
        assert_eq!(*cached_part(&mut part, at(1), || json!("two")), json!("one"));
        assert_eq!(*cached_part(&mut part, at(2), || json!("two")), json!("two"));
    }

    #[test]
    fn reads_share_one_copy_of_a_snapshot_part_until_it_changes() {
        std::thread::spawn(|| {
            let panes: fn(&Snapshot) -> &Value = |s| &s.panes;
            publish_panes(json!([{"pane":"p1","status":"idle"}]));
            let first = shared_part("panes", panes);
            assert!(std::sync::Arc::ptr_eq(&first, &shared_part("panes", panes)));
            assert!(!publish_panes(json!([{"pane":"p1","status":"idle"}])));
            assert!(std::sync::Arc::ptr_eq(&first, &shared_part("panes", panes)), "the same value is no change");
            publish_panes(json!([{"pane":"p1","status":"working"}]));
            assert_eq!(*shared_part("panes", panes), json!([{"pane":"p1","status":"working"}]));
        }).join().unwrap();
    }

    #[test]
    fn a_shared_value_redraws_only_the_copies_that_read_it() {
        std::thread::spawn(|| {
            let (theme, list) = (Rc::new(RefCell::new(LiveReads::default())), Rc::new(RefCell::new(LiveReads::default())));
            LIVE_READS.with_borrow_mut(|copies| {
                copies.insert(1, Rc::downgrade(&theme));
                copies.insert(2, Rc::downgrade(&list));
            });
            record(&theme, &shared_key("test.theme"));
            record(&list, &shared_key("test.sort"));
            // shared.json outlives a test run: start from values no earlier run left.
            let run = json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos().to_string());
            set_shared("test.theme".into(), run.clone());
            set_shared("test.sort".into(), run);
            assert!(snapshot_changed(1) && snapshot_changed(2));
            set_shared("test.sort".into(), json!("name"));
            assert!(!snapshot_changed(1), "a key it never read");
            assert!(snapshot_changed(2));
            set_shared("test.sort".into(), json!("name"));
            assert!(!snapshot_changed(2), "the same value is no change");
            set_shared("test.theme".into(), json!("dark"));
            assert!(snapshot_changed(1));
            assert!(!snapshot_changed(2));
        }).join().unwrap();
    }

    #[test]
    fn a_copy_coming_into_view_is_told_and_catches_up_on_missed_refreshes() {
        std::thread::spawn(|| {
            assert_eq!(set_shown_copies([1].into()), [1]);
            missed(2);
            let mut fresh = set_shown_copies([1, 2].into());
            fresh.sort();
            assert_eq!(fresh, [2], "already shown copies aren't news");
            assert!(is_shown(2));
            assert!(take_missed(2));
            assert!(!take_missed(2), "caught up once");
            assert!(set_shown_copies([2].into()).is_empty());
            assert!(!is_shown(1));
        }).join().unwrap();
    }

    /// An action can name the mod's own `<mod>.<command>`, as keys.json does:
    /// no args, and only its own (another mod's, a cloned mod's original's or
    /// a prefix-only one is no command).
    #[test]
    fn palette_actions_take_the_mods_own_commands() {
        let found = |id: &str, hint: Value| Found { id: id.into(), dir: PathBuf::new(), hint: serde_json::from_value(hint).unwrap(), hint_error: None, stamp: SystemTime::UNIX_EPOCH };
        let agents = found("agents", json!({ "actions": [
            { "title": "Fold all agents", "command": "agents.fold_all", "keywords": "collapse sub-agents" },
            { "title": "Null args", "command": "agents.unfold_all", "args": null },
            { "title": "With args", "command": "agents.fold_all", "args": { "x": 1 } },
            { "title": "Empty args", "command": "agents.fold_all", "args": {} },
            { "command": "agents.fold_all" },
            { "title": "Another mod's", "command": "ribbon.next" },
            { "title": "Prefix only", "command": "agents." },
            { "title": "Longer name", "command": "agentsx.fold_all" },
        ] }));
        let copy = found("agents-copy", json!({ "actions": [{ "title": "Original's", "command": "agents.fold_all" }] }));
        let (list, problems) = actions([&agents, &copy]);
        assert_eq!(list, vec![
            json!({ "mod": "agents", "title": "Fold all agents", "command": "agents.fold_all", "args": {}, "keywords": "collapse sub-agents" }),
            json!({ "mod": "agents", "title": "Null args", "command": "agents.unfold_all", "args": {}, "keywords": "" }),
            json!({ "mod": "agents", "title": "Empty args", "command": "agents.fold_all", "args": {}, "keywords": "" }),
        ]);
        assert_eq!(problems, vec![
            "mod agents: actions[2]: agents.fold_all: a mod's own command takes no args",
            "mod agents: actions[4]: needs a title",
            "mod agents: actions[5]: no command \"ribbon.next\"",
            "mod agents: actions[6]: no command \"agents.\"",
            "mod agents: actions[7]: no command \"agentsx.fold_all\"",
            "mod agents-copy: actions[0]: no command \"agents.fold_all\"",
        ]);
        let shipped = found("agents", serde_json::from_str(include_str!("../defaults/mods/agents/mod.json")).unwrap());
        let (list, problems) = actions([&shipped]);
        let commands: Vec<&str> = list.iter().map(|a| a["command"].as_str().unwrap()).collect();
        assert_eq!((commands, problems), (vec!["agents.fold_all", "agents.unfold_all"], vec![]), "the shipped agents mod's palette entries");
    }

    #[test]
    fn palette_actions_are_checked_and_malformed_ones_are_named_not_listed() {
        let found = |id: &str, hint: Value| Found { id: id.into(), dir: PathBuf::new(), hint: serde_json::from_value(hint).unwrap(), hint_error: None, stamp: SystemTime::UNIX_EPOCH };
        let acts = found("acts", json!({ "place": "none", "focusable": true, "actions": [
            { "title": "Send to all", "command": "mod.open", "args": { "props": { "target": "shells" } }, "keywords": "smart" },
            { "title": "Split", "command": "pane.split", "args": { "direction": "right" } },
            { "title": "Open other", "command": "mod.open", "args": { "name": "other" } },
            "split",
            { "command": "tab.new" },
            { "title": " ", "command": "tab.new" },
            { "title": "No command" },
            { "title": "Unknown", "command": "tab.fly" },
            { "title": "Bad args", "command": "tab.new", "args": ["x"] },
            { "title": "Misspelled", "command": "pane.split", "args": { "dir": "right" } },
            { "title": "Missing", "command": "pane.split" },
            { "title": "Null direction", "command": "pane.split", "args": { "direction": null } },
            { "title": "Blank direction", "command": "pane.split", "args": { "direction": " " } },
            { "title": "Blank name opens itself", "command": "mod.open", "args": { "name": "" } },
            { "title": "Bad props", "command": "mod.open", "args": { "props": "x" } },
            { "title": "Numbered", "command": "mod.open", "args": { "name": 5 } },
            { "title": "Bad keywords", "command": "tab.new", "keywords": ["a"] },
            { "title": "Bad target", "command": "tab.close", "args": { "id": 7 } },
        ] }));
        assert!(acts.hint.focusable && acts.hint.place.as_deref() == Some("none"), "a bad action costs the mod nothing else");
        let dir = std::env::temp_dir().join(format!("herdr-action-check-{}", std::process::id())).join("checked");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mod.json"), r#"{ "actions": [{ "title": "Go", "command": "mod.open" }, { "title": "Bad" }] }"#).unwrap();
        assert_eq!(mod_json_problems(&dir), vec!["mod checked: actions[1]: needs a command"], "herdr-app check reads the folder's own mod.json");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        assert_eq!(mod_json_problems(&dir.join("sub/..")), vec!["mod checked: actions[1]: needs a command"], "named by its folder however it's written");
        std::fs::write(dir.join("mod.json"), r#"{ "focusable": "yes", "actions": [] }"#).unwrap();
        assert!(mod_json_problems(&dir)[0].starts_with("mod checked: mod.json: invalid type"), "a mod.json Herdr can't read costs the mod its actions: check says so");
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
        let none = found("none", json!({ "place": "right" }));
        let odd = found("odd", json!({ "actions": { "title": "x" } }));
        let (list, problems) = actions([&acts, &none, &odd]);
        assert_eq!(list, vec![
            json!({ "mod": "acts", "title": "Send to all", "command": "mod.open", "args": { "name": "acts", "props": { "target": "shells" } }, "keywords": "smart" }),
            json!({ "mod": "acts", "title": "Split", "command": "pane.split", "args": { "direction": "right" }, "keywords": "" }),
            json!({ "mod": "acts", "title": "Open other", "command": "mod.open", "args": { "name": "other" }, "keywords": "" }),
            json!({ "mod": "acts", "title": "Blank name opens itself", "command": "mod.open", "args": { "name": "acts" }, "keywords": "" }),
        ]);
        assert_eq!(problems, vec![
            "mod acts: actions[3]: needs a title",
            "mod acts: actions[4]: needs a title",
            "mod acts: actions[5]: needs a title",
            "mod acts: actions[6]: needs a command",
            "mod acts: actions[7]: no command \"tab.fly\"",
            "mod acts: actions[8]: args: expected an object",
            "mod acts: actions[9]: pane.split: unknown argument(s): dir; valid arguments: direction, pane, argv, mod, props",
            "mod acts: actions[10]: pane.split: needs direction",
            "mod acts: actions[11]: pane.split: needs direction",
            "mod acts: actions[12]: pane.split: needs direction",
            "mod acts: actions[14]: mod.open: props: expected an object",
            "mod acts: actions[15]: mod.open: name: expected a mod's folder name",
            "mod acts: actions[16]: keywords: expected a string",
            "mod acts: actions[17]: tab.close: argument id must be a string (or null to omit it)",
            "mod odd: actions: expected a list",
        ]);
    }

    #[test]
    fn terminal_menu_items_are_checked_and_say_what_the_click_fills() {
        let found = |id: &str, hint: Value| Found { id: id.into(), dir: PathBuf::new(), hint: serde_json::from_value(hint).unwrap(), hint_error: None, stamp: SystemTime::UNIX_EPOCH };
        let mover = found("mover", json!({ "actions": [{ "title": "Palette", "command": "tab.new" }], "menu_items": [
            { "title": "Move to…", "command": "mod.open", "when": "pane", "icon": "move.svg" },
            { "title": "Split here", "command": "pane.split", "args": { "direction": "down" } },
            { "title": "Copy it", "command": "terminal.copy", "when": "selected" },
            { "title": "Focus it", "command": "pane.focus" },
            { "title": "That pane", "command": "pane.zoom", "args": { "pane": "p9" } },
            { "title": "Send back", "command": "terminal.park" },
            { "title": "Other mod", "command": "mod.open", "args": { "name": "other", "props": { "mode": "x" } }, "when": "cwd" },
            { "title": "Bad when", "command": "pane.zoom", "when": "agent" },
            { "title": "Listed when", "command": "pane.zoom", "when": ["pane"] },
            { "title": "Bad icon", "command": "pane.zoom", "icon": "../x.svg" },
            { "title": "Png icon", "command": "pane.zoom", "icon": "x.png" },
            { "title": "Still needs", "command": "pane.split" },
            { "command": "pane.zoom" },
        ] }));
        let plain = found("plain", json!({ "place": "right" }));
        let odd = found("odd", json!({ "menu_items": "x" }));
        let (list, problems) = menu_items([&mover, &plain, &odd]);
        assert_eq!(list, vec![
            json!({ "mod": "mover", "title": "Move to…", "command": "mod.open", "args": { "name": "mover" }, "when": "pane", "icon": "mods/mover/move.svg", "fill": ["props"] }),
            json!({ "mod": "mover", "title": "Split here", "command": "pane.split", "args": { "direction": "down" }, "when": null, "icon": null, "fill": ["pane"] }),
            json!({ "mod": "mover", "title": "Copy it", "command": "terminal.copy", "args": {}, "when": "selected", "icon": null, "fill": ["terminal"] }),
            json!({ "mod": "mover", "title": "Focus it", "command": "pane.focus", "args": {}, "when": null, "icon": null, "fill": ["pane", "terminal"] }),
            json!({ "mod": "mover", "title": "That pane", "command": "pane.zoom", "args": { "pane": "p9" }, "when": null, "icon": null, "fill": [] }),
            json!({ "mod": "mover", "title": "Send back", "command": "terminal.park", "args": {}, "when": null, "icon": null, "fill": ["terminal"] }),
            json!({ "mod": "mover", "title": "Other mod", "command": "mod.open", "args": { "name": "other", "props": { "mode": "x" } }, "when": "cwd", "icon": null, "fill": ["props"] }),
        ]);
        assert_eq!(problems, vec![
            "mod mover: menu_items[7]: when: expected one of \"selected\", \"cwd\", \"pane\"",
            "mod mover: menu_items[8]: when: expected one of \"selected\", \"cwd\", \"pane\"",
            "mod mover: menu_items[9]: icon: expected an .svg file in the mod's folder",
            "mod mover: menu_items[10]: icon: expected an .svg file in the mod's folder",
            "mod mover: menu_items[11]: pane.split: needs direction",
            "mod mover: menu_items[12]: needs a title",
            "mod odd: menu_items: expected a list",
        ]);
        assert_eq!(actions([&mover]).0.len(), 1, "a mod's palette actions and menu items are apart");
        let palette = found("p", json!({ "actions": [{ "title": "Send back", "command": "terminal.park" }] }));
        assert_eq!(actions([&palette]).1, vec!["mod p: actions[0]: terminal.park: needs terminal"], "only a menu item has a terminal to fill");
        assert!(actions([&mover]).1.is_empty() && menu_items([&found("x", json!({ "actions": [{ "title": "Go", "command": "tab.new" }] }))]).0.is_empty());

        let dir = std::env::temp_dir().join(format!("herdr-menu-check-{}", std::process::id())).join("checked");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mod.json"), r#"{ "actions": [{ "title": "Bad" }], "menu_items": [{ "title": "Go", "command": "tab.fly" }] }"#).unwrap();
        assert_eq!(mod_json_problems(&dir), vec!["mod checked: actions[0]: needs a command", "mod checked: menu_items[0]: no command \"tab.fly\""], "herdr-app check names both");
        std::fs::write(dir.join("mod.json"), r#"{ "place": 3, "menu_items": [{ "title": "Go", "command": "tab.fly" }] }"#).unwrap();
        let unreadable = mod_json_problems(&dir);
        assert!(unreadable.len() == 1 && unreadable[0].starts_with("mod checked: mod.json: invalid type"), "an unreadable mod.json is named once: {unreadable:?}");
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    /// A mod.json written for a later Herdr (a field this one doesn't know)
    /// keeps its place, actions and menu items.
    #[test]
    fn unknown_mod_json_fields_are_ignored() {
        let hint: Hint = serde_json::from_value(json!({ "place": "left", "someday": [1],
            "actions": [{ "title": "Go", "command": "tab.new" }], "menu_items": [{ "title": "Go", "command": "tab.new" }] })).unwrap();
        assert_eq!(hint.place.as_deref(), Some("left"));
        let found = Found { id: "later".into(), dir: PathBuf::new(), hint, hint_error: None, stamp: SystemTime::UNIX_EPOCH };
        let ((acts, act_problems), (items, item_problems)) = (actions([&found]), menu_items([&found]));
        assert!(acts.len() == 1 && items.len() == 1 && act_problems.is_empty() && item_problems.is_empty());
    }

    #[test]
    fn loading_copies_reclaims_closed_command_channels_without_dispatch() {
        std::thread::spawn(|| {
            let inst = Instance { name: "test".into(), props: Value::Null };
            let live = module(inst.clone(), 1);
            for copy in 2..102 {
                drop(module(inst.clone(), copy));
                COMMANDS.with_borrow(|commands| assert!(commands.len() <= 2));
            }
            drop(live);
            let last = module(inst, 102);
            COMMANDS.with_borrow(|commands| assert_eq!(commands.len(), 1));
            drop(last);
        }).join().unwrap();
    }

    #[test]
    fn keyboard_focus_is_opt_in_and_commands_only_reach_the_shown_copy() {
        for metadata in [json!({}), json!({"focusable": false})] {
            assert!(!serde_json::from_value::<Hint>(metadata).unwrap().focusable);
        }
        assert!(serde_json::from_value::<Hint>(json!({"focusable": true})).unwrap().focusable);

        let (first_tx, first) = async_channel::unbounded();
        let (second_tx, second) = async_channel::unbounded();
        let (hidden_tx, hidden) = async_channel::unbounded();
        COMMANDS.with_borrow_mut(|c| *c = vec![
            (1, "same-mod".into(), first_tx),
            (2, "same-mod".into(), second_tx),
            (3, "same-mod".into(), hidden_tx),
        ]);
        set_shown_copies([1, 2].into());
        assert!(send_command_to(2, "focus"));
        assert_eq!(second.try_recv().unwrap(), "focus");
        assert!(first.is_empty());
        assert!(hidden.is_empty());
        assert!(!send_command_to(3, "focus"));
        assert!(!send_command_to(99, "focus"));
        // Existing mod key bindings still broadcast to every shown copy.
        assert!(send_command("same-mod", "existing-key"));
        assert_eq!(first.try_recv().unwrap(), "existing-key");
        assert_eq!(second.try_recv().unwrap(), "existing-key");
        assert!(hidden.is_empty());
        COMMANDS.with_borrow_mut(Vec::clear);
        set_shown_copies(Default::default());
    }

    #[cfg(unix)]
    #[test]
    fn known_agents_uses_snapshot_and_refreshes_changed_commands() {
        let snapshot = vec![json!({"key": "claude", "run": ["herdr_issue57_not_installed"]})];
        let first = std::thread::spawn(move || known_agents(None, snapshot)).join().unwrap().unwrap();
        assert_eq!(first[0]["run"], json!(["herdr_issue57_not_installed"]));
        assert_eq!(first[0]["installed"], false);
        let snapshot = vec![json!({"key": "claude", "run": ["printf"]}), json!({"key": "claude-alt", "run": ["printf"]})];
        let next = std::thread::spawn(move || known_agents(None, snapshot)).join().unwrap().unwrap();
        assert_eq!(next.as_array().unwrap().len(), 2);
        assert_eq!(next[0]["installed"], true);
        assert_eq!(next[1]["key"], "claude-alt");
        assert_eq!(next[1]["installed"], true);
    }

    #[test]
    fn shared_bursts_persist_only_complete_snapshots_and_finish_at_final_state() {
        let root = if cfg!(target_os = "linux") { crate::test_tmp() } else { std::env::temp_dir() };
        let dir = root.join(format!("herdr-shared-stress-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared.json");
        let reading = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let reader = {
            let path = path.clone();
            let reading = reading.clone();
            std::thread::spawn(move || {
                while reading.load(std::sync::atomic::Ordering::Relaxed) {
                    match std::fs::read(&path) {
                        Ok(bytes) => { serde_json::from_slice::<Value>(&bytes).unwrap(); }
                        // Windows refuses to open a file while a rename replaces it. The reader
                        // got no bytes, so that is not a torn snapshot.
                        Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied => {}
                        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
                    }
                    std::thread::yield_now();
                }
            })
        };
        let expected = std::thread::spawn(move || {
            SHARED.with_borrow_mut(|s| *s = Some(SharedStore::load(path)));
            for i in 0..2000 {
                set_shared("changing".into(), json!("x".repeat(if i % 2 == 0 { 16000 } else { 3 })));
                set_shared("revision".into(), json!(i));
            }
            set_shared("final".into(), json!(true));
            finish_shared();
            with_shared(|s| s.values.clone())
        }).join().unwrap();
        reading.store(false, std::sync::atomic::Ordering::Relaxed);
        reader.join().unwrap();
        let actual: serde_json::Map<String, Value> = serde_json::from_slice(&std::fs::read(dir.join("shared.json")).unwrap()).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_shared_file_is_kept_before_starting_empty() {
        let root = if cfg!(target_os = "linux") { crate::test_tmp() } else { std::env::temp_dir() };
        let dir = root.join(format!("herdr-shared-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared.json");
        let corrupt = b"{\"kept\":true} old snapshot tail";
        std::fs::write(&path, corrupt).unwrap();
        let store = SharedStore::load(path.clone());
        assert!(store.values.is_empty());
        assert!(store.writable);
        assert!(!path.exists());
        let backup = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        assert!(backup.file_name().unwrap().to_string_lossy().starts_with("shared.json.corrupt-"));
        assert_eq!(std::fs::read(&backup).unwrap(), corrupt);
        crate::persist::write_atomic(&path, "{}").unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), corrupt);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn navigation_follows_mod_order() {
        let available = || vec!["t1".into(), "t2".into(), "t3".into()];
        AGENT_ORDER.with(|s| s.borrow_mut().clear());
        assert_eq!(agent_order(available()), available());
        AGENT_ORDER.with(|s| *s.borrow_mut() = vec!["t3".into(), "gone".into(), "t1".into(), "t3".into()]);
        assert_eq!(agent_order(available()), vec!["t3", "t1", "t2"]);
        assert!(agent_order(Vec::new()).is_empty());
        AGENT_ORDER.with(|s| *s.borrow_mut() = vec!["t2".into(), "t1".into(), "t3".into()]);
        assert_eq!(agent_order(available()), vec!["t2", "t1", "t3"]);
        AGENT_ORDER.with(|s| s.borrow_mut().clear());
    }

    #[test]
    fn an_added_file_makes_a_shipped_mod_edited() {
        let dir = std::env::temp_dir().join(format!("herdr-app-shipped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        std::fs::write(dir.join("main.js"), "a").unwrap();
        std::fs::write(dir.join("icons/x.svg"), "b").unwrap();
        let files: Files = [("main.js".to_owned(), fingerprint(b"a")), ("icons/x.svg".to_owned(), fingerprint(b"b"))].into();
        assert!(as_shipped(&dir, &files));
        std::fs::write(dir.join("gpui-kit.d.ts"), "generated").unwrap();
        std::fs::write(dir.join("jsconfig.json"), "generated").unwrap();
        assert!(as_shipped(&dir, &files));
        std::fs::write(dir.join(".DS_Store"), "finder").unwrap();
        std::fs::create_dir(dir.join(".vscode")).unwrap();
        std::fs::write(dir.join(".vscode/settings.json"), "{}").unwrap();
        assert!(as_shipped(&dir, &files));
        std::fs::write(dir.join("mine.js"), "c").unwrap();
        assert!(!as_shipped(&dir, &files));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cloned_mods_rewrite_assets_and_self_overlays_and_reject_nonobject_metadata() {
        let root = std::env::temp_dir().join(format!("herdr-mod-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("original")).unwrap();
        std::fs::write(root.join("original/mod.json"), r#"{ "actions": [{ "title": "Fold", "command": "original.fold_all", "keywords": "original" }] }"#).unwrap();
        std::fs::write(root.join("original/main.js"), r#"const ASSETS = "mods/original"; const icon = 'mods/original/icon.svg'; openOverlay({ mod: "original" });"#).unwrap();
        std::fs::write(root.join("original/icon.svg"), "<svg/>").unwrap();
        clone_mod_at(&root, "original", "copy").unwrap();
        let text = std::fs::read_to_string(root.join("copy/main.js")).unwrap();
        assert!(text.contains("\"mods/copy\"") && text.contains("'mods/copy/icon.svg'") && text.contains("mod: \"copy\""));
        assert_eq!(std::fs::read_to_string(root.join("copy/icon.svg")).unwrap(), "<svg/>");
        let meta: Value = serde_json::from_str(&std::fs::read_to_string(root.join("copy/mod.json")).unwrap()).unwrap();
        assert_eq!(meta["actions"], json!([{ "title": "Fold", "command": "copy.fold_all", "keywords": "original" }]), "its own commands follow the copy");
        assert!(clone_mod_at(&root, "original", "copy").is_err());
        for invalid in ["[]", "42", "true", "null", "\"text\""] {
            std::fs::write(root.join("original/mod.json"), invalid).unwrap();
            assert!(clone_mod_at(&root, "original", "bad").is_err());
            assert!(!root.join("bad").exists());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layout_formatting_is_not_an_edit_and_rescued_clicks_still_apply() {
        use crate::layout::{Node, Size};
        use crate::layout_sync::State;
        let root = std::env::temp_dir().join(format!("herdr-layout-click-rescue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("layouts");
        std::fs::create_dir_all(dir.join(".state")).unwrap();
        std::fs::create_dir_all(dir.join(".base")).unwrap();
        let shipped = r#"{"id":"root","type":"block","content":{"mod":"files"},"size":{"px":360}}"#;
        // No record: formatting, BOM and whole-number floats are not edits.
        std::fs::write(dir.join("main.json"), "\u{feff}{\r\n \"type\":\"block\", \"id\":\"root\", \"size\":{\"px\":360.0}, \"content\":{\"mod\":\"files\"}}").unwrap();
        sync_layouts(&root, &[("main", shipped)]).unwrap();
        assert!(!dir.join("main-edited.json").exists());
        assert!(!dir.join(".base").exists());
        let mut file: Node = serde_json::from_str(shipped).unwrap();
        file.size = Some(Size::Px { px: 400.0 });
        std::fs::write(dir.join("main.json"), crate::layout_sync::text_of(&file)).unwrap();
        let mut shown = file.clone();
        shown.size = Some(Size::Px { px: 500.0 });
        let mut state = State::between(&file, &shown);
        state.file = serde_json::to_value(&file).ok();
        std::fs::write(dir.join(".state/main.json"), serde_json::to_string(&state).unwrap()).unwrap();
        sync_layouts(&root, &[("main", shipped)]).unwrap();
        let mut rescued: Node = serde_json::from_str(&std::fs::read_to_string(dir.join("main-edited.json")).unwrap()).unwrap();
        let mut clicks: State = serde_json::from_str(&std::fs::read_to_string(dir.join(".state/main-edited.json")).unwrap()).unwrap();
        clicks.forget_since_saved(&rescued);
        clicks.apply(&mut rescued);
        assert_eq!(rescued.size, Some(Size::Px { px: 500.0 }));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_0_1_x_layout_is_kept_aside_quietly_not_rescued() {
        let root = std::env::temp_dir().join(format!("herdr-layout-0-1-x-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("layouts");
        std::fs::create_dir_all(&dir).unwrap();
        // 0.1.x wrote no record of what it installed.
        std::fs::write(dir.join("main.json"), r#"{"id":"root","type":"block","content":{"mod":"old"}}"#).unwrap();
        take_sync_notes();
        sync_layouts(&root, &[("main", r#"{"id":"root","type":"block","content":{"mod":"new"}}"#)]).unwrap();
        assert!(std::fs::read_to_string(dir.join("main.json")).unwrap().contains("new"));
        let aside = dir.join(format!("main.json.before-{}", env!("CARGO_PKG_VERSION")));
        assert!(std::fs::read_to_string(aside).unwrap().contains("old"));
        assert!(!dir.join("main-edited.json").exists());
        assert!(take_sync_notes().is_empty(), "no toast for a file nobody can say was edited");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layouts_restore_preserve_edits_clicks_and_drop_only_untouched() {
        let root = std::env::temp_dir().join(format!("herdr-layout-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("layouts");
        sync_layouts(&root, &[("a", "a1"), ("b", "b1"), ("c", "c1")]).unwrap();
        std::fs::create_dir_all(dir.join(".state")).unwrap();
        std::fs::write(dir.join(".state/a.json"), "clicks").unwrap();
        std::fs::write(dir.join("a.json"), "mine").unwrap();
        std::fs::write(dir.join("b.json"), "kept").unwrap();
        sync_layouts(&root, &[("a", "a2")]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "a2");
        assert_eq!(std::fs::read_to_string(dir.join("a-edited.json")).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(dir.join("b.json")).unwrap(), "kept");
        assert!(!dir.join("c.json").exists());
        assert_eq!(std::fs::read_to_string(dir.join(".state/a.json")).unwrap(), "clicks");
        std::fs::write(dir.join("a.json"), "again").unwrap();
        sync_layouts(&root, &[("a", "a2")]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a-edited-2.json")).unwrap(), "again");
        assert_eq!(std::fs::read_to_string(dir.join("a-edited.json")).unwrap(), "mine");
        std::fs::remove_file(dir.join("a.json")).unwrap();
        sync_layouts(&root, &[("a", "a3")]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "a3");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_mods_restore_shipped_and_move_edits_to_clones() {
        let root = std::env::temp_dir().join(format!("herdr-app-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (mods, record) = (root.join("mods"), root.join("installed.json"));
        let read = |p: &str| std::fs::read_to_string(mods.join(p)).unwrap_or_default();

        let v1: &[(&str, &[u8])] = &[("a/main.js", b"a1"), ("a/old.js", b"x"), ("b/main.js", b"b1"), ("c/main.js", b"c1")];
        sync_defaults(&mods, &record, v1).unwrap();
        assert_eq!(read("a/main.js"), "a1");
        let before = std::fs::metadata(mods.join("a/main.js")).unwrap().modified().unwrap();
        sync_defaults(&mods, &record, v1).unwrap();
        assert_eq!(std::fs::metadata(mods.join("a/main.js")).unwrap().modified().unwrap(), before, "untouched mods are not rewritten");

        std::fs::write(mods.join("b/main.js"), "mine").unwrap();
        std::fs::remove_dir_all(mods.join("c")).unwrap();
        let v2: &[(&str, &[u8])] = &[("a/main.js", b"a2"), ("b/main.js", b"b2"), ("c/main.js", b"c2")];
        sync_defaults(&mods, &record, v2).unwrap();
        assert_eq!(read("a/main.js"), "a2", "untouched mods update");
        assert!(!mods.join("a/old.js").exists(), "files the update dropped go");
        assert_eq!(read("b/main.js"), "b2", "edited mods are restored");
        assert_eq!(read("b-edited/main.js"), "mine", "edits become a clone");
        assert_eq!(read("c/main.js"), "c2", "deleted mods are restored");
        std::fs::write(mods.join("b/main.js"), "changed again").unwrap();

        // No longer shipped: untouched ones go, edited ones stay, and so do
        // ones someone added a file to.
        let v3: &[(&str, &[u8])] = &[("c/main.js", b"c2"), ("d/main.js", b"d1")];
        sync_defaults(&mods, &record, v3).unwrap();
        std::fs::write(mods.join("d/notes.md"), "my notes").unwrap();
        let v4: &[(&str, &[u8])] = &[("c/main.js", b"c2")];
        sync_defaults(&mods, &record, v4).unwrap();
        assert!(!mods.join("a").exists(), "untouched dropped defaults go");
        assert_eq!(read("b/main.js"), "changed again", "edited dropped defaults stay");
        assert_eq!(read("b-edited/main.js"), "mine", "clones stay untouched");
        assert_eq!(read("d/notes.md"), "my notes", "a dropped default with an added file stays");

        std::fs::remove_dir_all(&root).unwrap();
    }
}
