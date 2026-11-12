//! `anyr stats [report]` against a local scripted server: which route and
//! credential each report uses, the flags it passes, and its output forms.

mod common;
use common::serve;
use std::process::{Command, Stdio};
use std::time::Duration;

const MGMT: &str = "ak_test_management_key_0001";
const SUMMARY: &str = r#"{"total_requests":12,"total_tokens":3400,"total_cost":0.0123,"avg_latency_ms":850,"cache_hit_rate":0.25,"cost_savings":0,"period":{"start":"a","end":"b"}}"#;
const MODELS: &str = r#"{"period":{"key":"this_month"},"models":[{"rank":1,"model_id":"x/one","requests":9,"tokens":100,"cost":0.5,"success_rate":1}],"buckets":[],"totals":{}}"#;

/// Run `anyr <args> --base-url <base>`, with the management key when `mk`.
fn anyr(base: &str, args: &[&str], mk: bool) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anyr"));
    cmd.env("ANYR_NO_UPDATE", "1")
        .env("ANYR_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .env("ANYROUTER_API_KEY", "sk-ar-v1-testkey0123456789")
        .env_remove("ANYROUTER_MANAGEMENT_KEY")
        .env(
            "ANYROUTER_HOME",
            std::env::temp_dir().join("anyr-stats-test-empty"),
        )
        .args(args)
        .args(["--base-url", base])
        .stdin(Stdio::null());
    if mk {
        cmd.env("ANYROUTER_MANAGEMENT_KEY", MGMT);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn bare_stats_is_the_summary_read_with_the_management_key() {
    // WHY: the analytics routes reject sk-ar keys; sending one would get a
    // 401 and a misleading "sign in again".
    let (base, rx) = serve(vec![(200, SUMMARY.into())]);
    let (code, out, err) = anyr(&base, &["stats"], true);
    assert_eq!(code, 0, "{err}");
    let seen = rx.recv().unwrap();
    assert_eq!(seen.path, "/api/v1/analytics/summary");
    assert!(seen.auth.contains(MGMT), "{}", seen.auth);
    // Piped: one TSV row in column order.
    assert_eq!(out, "12\t3400\t0.0123\t850\t0.25\n");
}

#[test]
fn json_is_the_servers_body_unchanged() {
    let (base, _rx) = serve(vec![(200, SUMMARY.into())]);
    let (code, out, err) = anyr(&base, &["stats", "summary", "--json"], true);
    assert_eq!(code, 0, "{err}");
    let got: serde_json::Value = serde_json::from_str(&out).unwrap();
    let want: serde_json::Value = serde_json::from_str(SUMMARY).unwrap();
    assert_eq!(got, want);
}

#[test]
fn each_report_sends_only_the_params_its_route_reads() {
    let (base, rx) = serve(vec![
        (200, r#"{"daily":[]}"#.into()),
        (200, r#"{"data":[]}"#.into()),
        (200, r#"{"points":[]}"#.into()),
        (200, SUMMARY.into()),
    ]);
    for (args, path) in [
        (
            &[
                "stats",
                "usage",
                "--from",
                "2026-11-01",
                "--to",
                "2026-11-05",
            ][..],
            "/api/v1/analytics/usage-range?from=2026-11-01&to=2026-11-05",
        ),
        (
            &["stats", "keys", "--days", "30"],
            "/api/v1/analytics/keys-usage?days=30",
        ),
        (
            &["stats", "burn", "--range", "12m"],
            "/api/v1/analytics/token-burn?range=12m",
        ),
        (
            &["stats", "summary", "--period", "7d"],
            "/api/v1/analytics/summary?period=7d",
        ),
    ] {
        let (code, _, err) = anyr(&base, args, true);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(rx.recv().unwrap().path, path);
    }
}

#[test]
fn public_reports_need_no_management_key() {
    let (base, rx) = serve(vec![(200, MODELS.into())]);
    let (code, out, err) = anyr(
        &base,
        &["stats", "models", "--period", "this_month", "--limit", "5"],
        false,
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        rx.recv().unwrap().path,
        "/api/v1/analytics/top-models?period=this_month&limit=5"
    );
    assert_eq!(out, "1\tx/one\t9\t100\t0.5\t1\n");
}

#[test]
fn private_reports_without_a_management_key_exit_4_and_send_nothing() {
    let (base, rx) = serve(vec![(200, SUMMARY.into())]);
    for report in ["summary", "usage", "keys"] {
        let (code, _, err) = anyr(&base, &["stats", report], false);
        assert_eq!(code, 4, "{report}: {err}");
        assert!(err.contains("read:analytics"), "{err}");
    }
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn bad_reports_flags_and_values_exit_2_and_send_nothing() {
    // WHY: the server silently falls back to its default on an unknown value,
    // so a typo would print the wrong window with no error.
    let (base, rx) = serve(vec![(200, SUMMARY.into())]);
    for args in [
        &["stats", "nope"][..],
        &["stats", "summary", "extra"],
        &["stats", "burn", "--range", "1y"],
        &["stats", "burn", "--days", "3"],
        &["stats", "summary", "--period", "this_month"],
        &["stats", "keys", "--days", "0"],
        &["stats", "models", "--limit", "x"],
    ] {
        let (code, _, err) = anyr(&base, args, true);
        assert_eq!(code, 2, "{args:?}: {err}");
    }
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn server_errors_keep_their_exit_codes() {
    for (status, want) in [(401, 4), (404, 3), (402, 8)] {
        let (base, _rx) = serve(vec![(status, r#"{"error":{"message":"nope"}}"#.into())]);
        let (code, _, err) = anyr(&base, &["stats", "--json"], true);
        assert_eq!(code, want, "HTTP {status}: {err}");
    }
}
