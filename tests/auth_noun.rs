//! `auth` as the one account noun: `list|add|rename|remove` run the same code
//! as `account`, and `auth status --local` reads config.yaml only.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

mod common;

const KEY: &str = "sk-ar-v1-authnoun0123456789abcdefgh";
const MGMT: &str = "ak_authnoun0123456789abcdef";

fn anyr(home: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env_remove("ANYROUTER_API_KEY")
        .env_remove("ANYROUTER_PROFILE")
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .env_remove("ANYR_OUTPUT")
        .env("HOME", home)
        .env("ANYROUTER_HOME", home)
        .env("ANYR_NO_UPDATE", "1")
        .env("ANYR_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .args(args)
        .stdin(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Two accounts; `base_url` points nowhere, so any request would fail.
fn two_accounts(tag: &str) -> std::path::PathBuf {
    let home = common::unique_temp_dir(&format!("anyr-auth-noun-{tag}"));
    std::fs::write(
        home.join("config.yaml"),
        format!(
            "active_profile: default\nprofiles:\n  default:\n    api_key: {KEY}\n    management_key: {MGMT}\n    base_url: http://127.0.0.1:9/api\n  work:\n    base_url: http://127.0.0.1:9/api\n"
        ),
    )
    .unwrap();
    home
}

#[test]
fn status_local_lists_every_account_offline_without_the_key() {
    let home = two_accounts("local");
    let (code, out, err) = anyr(&home, &["auth", "status", "--local", "--json"], &[]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["active"], "default");
    let rows = v["profiles"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "default");
    assert_eq!(rows[0]["active"], true);
    assert_eq!(rows[0]["key_source"], "config");
    assert_eq!(rows[0]["has_management_key"], true);
    assert_eq!(rows[0]["key_masked"], "sk-ar-v1-authn…efgh");
    assert_eq!(
        (&rows[1]["key_source"], &rows[1]["active"]),
        (&"none".into(), &false.into())
    );
    assert!(!out.contains(KEY) && !out.contains(MGMT), "{out}");

    // The env key wins for the active account, and says so.
    let (_, out, _) = anyr(
        &home,
        &["auth", "status", "--local", "--json"],
        &[("ANYROUTER_API_KEY", "sk-ar-v1-fromenv0123456789abcdef")],
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["profiles"][0]["key_source"], "env");
}

#[test]
fn status_local_exits_0_with_no_config_and_fails_on_an_unreadable_one() {
    let home = common::unique_temp_dir("anyr-auth-noun-empty");
    let (code, out, _) = anyr(&home, &["auth", "status", "--local", "--json"], &[]);
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["profiles"],
        serde_json::json!([])
    );
    std::fs::create_dir_all(home.join("config.yaml")).unwrap();
    let (code, _, _) = anyr(&home, &["auth", "status", "--local", "--json"], &[]);
    assert_ne!(code, 0);
}

#[test]
fn plain_auth_status_json_keeps_its_fields_and_adds_profiles() {
    // WHY: additive only; scripts that read whoami fields keep working.
    let home = two_accounts("whoami");
    let (code, out, err) = anyr(&home, &["auth", "status", "--json"], &[]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    for field in [
        "signed_in",
        "account",
        "key_masked",
        "config_path",
        "base_url",
        "agents",
    ] {
        assert!(v.get(field).is_some(), "{field}: {out}");
    }
    assert_eq!(v["profiles"].as_array().unwrap().len(), 2, "{out}");
    // Signed out still exits 4, as before.
    let empty = common::unique_temp_dir("anyr-auth-noun-out");
    let (code, _, _) = anyr(&empty, &["auth", "status", "--json"], &[]);
    assert_eq!(code, 4);
}

#[test]
fn auth_account_verbs_run_the_account_code() {
    let home = two_accounts("verbs");
    let (_, auth, _) = anyr(&home, &["auth", "list", "--json"], &[]);
    let (_, account, _) = anyr(&home, &["account", "list", "--json"], &[]);
    assert_eq!(auth, account);
    let (code, _, err) = anyr(&home, &["auth", "rename", "work", "job"], &[]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = anyr(&home, &["auth", "remove", "job"], &[]);
    assert_eq!(code, 0, "{err}");
    let (code, _, _) = anyr(&home, &["auth", "remove", "job"], &[]);
    assert_eq!(
        code, 3,
        "removing a missing account is not found, as in account"
    );
    let (_, out, _) = anyr(&home, &["auth", "list", "--json"], &[]);
    assert_eq!(
        serde_json::from_str::<Value>(&out)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
