//! Mods' own settings. A mod declares them in `mod.schema.json` beside its
//! mod.json, as JSON Schema under `properties.settings.properties` (`title`,
//! `description`, `type`, `default`, `enum`, `minimum`/`maximum`, `examples`,
//! and Better Config's `x-labels`, `x-step`, `x-unit`, `format: duration|size`).
//! The person's values live in settings.json's `mods.<name>`, never in the
//! mod's folder, and are merged over the schema's defaults: a default is
//! declared only in the schema.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// A mod's folder from its loaded name (`frame-<name>` is the frame's own).
pub fn dir_of(name: &str) -> PathBuf {
    match name.strip_prefix("frame-") {
        Some(frame) => crate::mods::frame_dir().join(frame),
        None => crate::mods::mods_dir().join(name),
    }
}

/// The schema beside a mod's mod.json, if it has a readable one.
pub fn read_schema(dir: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join("mod.schema.json")).ok()?).ok()
}

/// The settings a schema declares, by key; none without them.
pub fn declared(schema: &Value) -> Map<String, Value> {
    schema.pointer("/properties/settings/properties").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// Each declared setting's `default`.
pub fn defaults(declared: &Map<String, Value>) -> Map<String, Value> {
    declared.iter().filter_map(|(key, spec)| Some((key.clone(), spec.get("default")?.clone()))).collect()
}

// The storage seam: `stored` (read) and `write` are the only code that knows
// where the person's values live (settings.json's `mods.<name>`, and the
// top-level keys some settings moved from). Everything else takes the map
// `stored` returns, by mod name then key.

/// Mod settings that were top-level settings.json keys: (mod, key, old key).
/// Read while the mod's own is unset; removed when it is written.
const MOVED_FROM: &[(&str, &str, &str)] = &[("pane", "header_info", "pane_header_info"), ("agents", "card_info", "agents_card_info")];

/// The person's values for every mod, by mod name then key, as written.
pub fn stored(person: &crate::settings::Person) -> Map<String, Value> {
    let mut mods: Map<String, Value> = person.mods.iter().filter(|(_, v)| v.is_object()).map(|(k, v)| (k.clone(), v.clone())).collect();
    let legacy = |old: &str| match old {
        "pane_header_info" => person.pane_header_info.clone(),
        "agents_card_info" => person.agents_card_info.clone(),
        _ => None,
    };
    for (name, key, old) in MOVED_FROM {
        let Some(value) = legacy(old) else { continue };
        let own = mods.entry(name.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(own) = own {
            own.entry(key.to_string()).or_insert(Value::String(value));
        }
    }
    mods
}

/// Writes one of mod `name`'s settings to settings.json: `None` (or the
/// schema's default) removes it, so the default applies; the top-level key
/// it moved from goes either way. Other keys are kept as written.
pub fn write(name: &str, key: &str, value: Option<Value>) -> Result<(), String> {
    let default = read_schema(&dir_of(name)).and_then(|s| declared(&s).get(key)?.get("default").cloned());
    let value = value.filter(|v| Some(v) != default.as_ref());
    let path = crate::settings::path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e.to_string()),
    };
    let mut raw: Map<String, Value> = serde_json::from_str(&text).map_err(|e| format!("settings.json has errors: {e}"))?;
    edit(&mut raw, name, key, value);
    crate::persist::write_atomic(&path, serde_json::to_string_pretty(&raw).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())
}

fn edit(raw: &mut Map<String, Value>, name: &str, key: &str, value: Option<Value>) {
    for (_, _, old) in MOVED_FROM.iter().filter(|(n, k, _)| *n == name && *k == key) {
        raw.remove(*old);
    }
    let mods = raw.entry("mods").or_insert_with(|| Value::Object(Map::new()));
    if !mods.is_object() {
        *mods = Value::Object(Map::new());
    }
    let mods = mods.as_object_mut().unwrap();
    let own = mods.entry(name).or_insert_with(|| Value::Object(Map::new()));
    if !own.is_object() {
        *own = Value::Object(Map::new());
    }
    let own_map = own.as_object_mut().unwrap();
    match value {
        Some(v) => {
            own_map.insert(key.to_owned(), v);
        }
        None => {
            own_map.remove(key);
        }
    }
    if own_map.is_empty() {
        mods.remove(name);
    }
    if mods.is_empty() {
        raw.remove("mods");
    }
}

/// One of mod `name`'s values in a map from `stored`.
pub fn own<'a>(mods: &'a Map<String, Value>, name: &str, key: &str) -> Option<&'a Value> {
    mods.get(name)?.get(key)
}

/// A mod text setting the app reads itself: None (the default) when unset or not text.
pub fn text(mods: &Map<String, Value>, name: &str, key: &str) -> Option<String> {
    own(mods, name, key)?.as_str().map(str::to_owned)
}

/// Why a setting the app reads itself was set to something it can't use.
pub fn problems(mods: &Map<String, Value>) -> Vec<String> {
    MOVED_FROM
        .iter()
        .filter_map(|(name, key, _)| own(mods, name, key).filter(|v| !v.is_string()).map(|v| format!("mods.{name}.{key} {v} isn't text: using its default")))
        .collect()
}

/// What `modSettings()` gives mod `name`: the schema's defaults with the
/// person's values over them.
pub fn merged(defaults: &Map<String, Value>, mods: &Map<String, Value>, name: &str) -> Value {
    let mut values = defaults.clone();
    if let Some(Value::Object(own)) = mods.get(name) {
        values.extend(own.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    Value::Object(values)
}

/// What `modSchemas()` gives the Settings window: each mod folder whose schema
/// declares settings, by name, with its title (mod.json's, else the folder's)
/// and the declared settings in schema order.
pub fn schemas(found: &[crate::mods::Found]) -> Value {
    found
        .iter()
        .filter(|f| !f.id.starts_with("frame-"))
        .filter_map(|f| {
            let declared = declared(&read_schema(&f.dir)?);
            if declared.is_empty() {
                return None;
            }
            let title = mod_title(&f.dir).unwrap_or_else(|| f.id.clone());
            Some(serde_json::json!({ "mod": f.id, "title": title, "settings": declared }))
        })
        .collect()
}

fn mod_title(dir: &Path) -> Option<String> {
    let meta: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("mod.json")).ok()?).ok()?;
    meta.get("title")?.as_str().map(str::to_owned)
}

/// A shipped mod's declared default, read from the schema built in.
pub fn shipped_default(schema: &str, key: &str) -> Value {
    let schema: Value = serde_json::from_str(schema).expect("a shipped mod.schema.json parses");
    declared(&schema).get(key).and_then(|s| s.get("default")).cloned().unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PANE: &str = include_str!("../defaults/mods/pane/mod.schema.json");
    const AGENTS: &str = include_str!("../defaults/mods/agents/mod.schema.json");

    #[test]
    fn values_merge_over_the_schema_defaults() {
        let schema = json!({ "properties": { "settings": { "properties": { "a": { "type": "string", "default": "x" }, "b": { "type": "boolean", "default": true }, "c": { "type": "string" } } } } });
        let defaults = defaults(&declared(&schema));
        assert_eq!(Value::Object(defaults.clone()), json!({ "a": "x", "b": true }), "a setting without a default has no value");
        let mods = json!({ "m": { "b": false, "c": "set" }, "other": { "a": "no" } });
        let mods = mods.as_object().unwrap();
        assert_eq!(merged(&defaults, mods, "m"), json!({ "a": "x", "b": false, "c": "set" }));
        assert_eq!(merged(&defaults, mods, "none"), json!({ "a": "x", "b": true }), "another mod's values stay its own");
        assert_eq!(own(mods, "m", "c"), Some(&json!("set")));
        assert_eq!(own(mods, "m", "a"), None);
    }

    /// Writing keeps other keys, removes the key a setting moved from, and
    /// leaves no empty sections behind; reading falls back to the old key.
    #[test]
    fn writes_go_under_mods_and_reads_fall_back_to_the_old_keys() {
        let mut raw = json!({ "zoom": 1.1, "pane_header_info": "{state}" }).as_object().unwrap().clone();
        edit(&mut raw, "pane", "header_info", Some(json!("{branch}")));
        assert_eq!(Value::Object(raw.clone()), json!({ "zoom": 1.1, "mods": { "pane": { "header_info": "{branch}" } } }));
        edit(&mut raw, "pane", "header_info", None);
        assert_eq!(Value::Object(raw.clone()), json!({ "zoom": 1.1 }));
        edit(&mut raw, "x", "a", Some(json!(true)));
        edit(&mut raw, "x", "b", Some(json!(2)));
        edit(&mut raw, "x", "a", None);
        assert_eq!(raw["mods"], json!({ "x": { "b": 2 } }));
        let person: crate::settings::Person = serde_json::from_value(json!({ "pane_header_info": "{state}", "agents_card_info": "", "mods": { "agents": { "card_info": "{space}" } } })).unwrap();
        let mods = stored(&person);
        assert_eq!(text(&mods, "pane", "header_info").as_deref(), Some("{state}"), "an older settings.json keeps its value");
        assert_eq!(text(&mods, "agents", "card_info").as_deref(), Some("{space}"), "the new key wins");
    }

    #[test]
    fn a_folder_without_a_schema_declares_nothing() {
        let dir = std::env::temp_dir().join(format!("herdr-mod-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_schema(&dir), None);
        std::fs::write(dir.join("mod.schema.json"), "{ broken").unwrap();
        assert_eq!(read_schema(&dir), None, "an unreadable schema is no schema");
        assert!(declared(&json!({ "properties": {} })).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The shipped schemas declare the two settings their Rust readers use,
    /// with the defaults the app had before they moved.
    #[test]
    fn shipped_schemas_declare_header_and_card_info() {
        assert_eq!(shipped_default(PANE, "header_info"), json!(""));
        assert_eq!(shipped_default(AGENTS, "card_info"), json!("@{remote} · {space}"));
        for schema in [PANE, AGENTS] {
            let v: Value = serde_json::from_str(schema).unwrap();
            for (key, spec) in declared(&v) {
                assert!(spec.get("title").and_then(Value::as_str).is_some(), "{key} has a title for Settings");
                assert!(spec.get("description").and_then(Value::as_str).is_some(), "{key} has a description for Settings");
            }
        }
        for mod_json in [include_str!("../defaults/mods/pane/mod.json"), include_str!("../defaults/mods/agents/mod.json")] {
            let v: Value = serde_json::from_str(mod_json).unwrap();
            assert_eq!(v["$schema"], "./mod.schema.json", "mod.json points editors at its schema");
            assert!(v.get("settings").is_none(), "values never live in mod.json");
        }
    }
}
