//! `anyr schema`: the typed command map agents read before calling anyr.

use std::process::{Command, Stdio};

use serde_json::Value;

fn anyr(args: &[&str]) -> (i32, String, String) {
    let home = std::env::temp_dir().join(format!("anyr-schema-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_anyr"))
        .env_clear()
        .env("HOME", &home)
        .env("ANYROUTER_HOME", &home)
        .env("ANYR_NO_UPDATE", "1")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn one_path_prints_its_typed_entry_without_json_flag() {
    let (code, out, err) = anyr(&["schema", "keys", "create"]);
    assert_eq!(code, 0, "{err}");
    let e: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(e["schema"], "anyr.schema/v2");
    assert_eq!(e["name"], "keys create");
    assert!(e["flags"][0]["type"].is_string(), "{out}");
    assert_eq!(e["auth"], "sk");
    assert_eq!(e["mutating"], true);
}

#[test]
fn the_whole_map_covers_every_command_of_commands_json() {
    let (code, out, _) = anyr(&["schema"]);
    assert_eq!(code, 0);
    let schema: Value = serde_json::from_str(&out).unwrap();
    let (_, tree, _) = anyr(&["commands", "--json"]);
    let tree: Value = serde_json::from_str(&tree).unwrap();
    assert_eq!(
        tree["schema"], "anyr.commands/v1",
        "the old map keeps its id"
    );
    let names: Vec<&Value> = schema["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["name"])
        .collect();
    for c in tree["commands"].as_array().unwrap() {
        assert!(names.contains(&&c["name"]), "{}", c["name"]);
    }
}

#[test]
fn an_unknown_path_is_a_json_not_found_error() {
    // Agents read stderr as JSON for every schema call, --json or not.
    let (code, out, err) = anyr(&["schema", "keys", "nope"]);
    assert_eq!(code, 3, "{out}{err}");
    assert!(out.is_empty());
    let v: Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"]["code"], "not_found");
    assert_eq!(v["error"]["exit"], 3);
}
