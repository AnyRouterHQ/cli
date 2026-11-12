//! `anyr credits [balance|history]` against a local scripted server: the same
//! requests and output as `usage` and `transactions`.

mod common;
use common::{run, serve};
use std::time::Duration;

const CREDITS: &str = r#"{"data":{"balance":12.5,"total_credits":20,"total_usage":7.5}}"#;
const LEDGER: &str = r#"{"data":[{"created_at":"2026-10-24T01:51:16Z","type":"purchase","amount":10,"balance_after":12.5,"description":"Top up"}]}"#;

#[test]
fn credits_and_credits_balance_print_what_usage_prints() {
    // WHY: credits is the new name; scripts moving to it must get the same
    // JSON byte for byte.
    let (base, rx) = serve(vec![
        (200, CREDITS.into()),
        (200, CREDITS.into()),
        (200, CREDITS.into()),
    ]);
    let (code, usage, err) = run(&base, &["usage", "--json"], None);
    assert_eq!(code, 0, "{err}");
    for args in [
        &["credits", "--json"][..],
        &["credits", "balance", "--json"],
    ] {
        let (code, out, err) = run(&base, args, None);
        assert_eq!(code, 0, "{args:?}: {err}");
        assert_eq!(out, usage, "{args:?}");
    }
    for _ in 0..3 {
        assert_eq!(rx.recv().unwrap().path, "/api/v1/credits");
    }
}

#[test]
fn credits_history_is_transactions_with_its_flags() {
    let (base, rx) = serve(vec![(200, LEDGER.into())]);
    let (code, out, err) = run(
        &base,
        &["credits", "history", "--type", "purchase", "--limit", "5"],
        None,
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("+$10.00"), "{out}");
    assert_eq!(
        rx.recv().unwrap().path,
        "/api/v1/credits/transactions?limit=5&type=purchase"
    );
}

#[test]
fn a_flag_of_the_other_verb_or_an_unknown_verb_is_exit_2_and_sends_nothing() {
    let (base, rx) = serve(vec![(200, CREDITS.into())]);
    for args in [
        &["credits", "--limit", "5"][..],
        &["credits", "history", "--no-detail"],
        &["credits", "topup"],
        &["credits", "balance", "extra"],
    ] {
        let (code, _, err) = run(&base, args, None);
        assert_eq!(code, 2, "{args:?}: {err}");
    }
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn server_errors_keep_their_exit_codes() {
    for (status, want) in [(401, 4), (402, 8), (404, 3)] {
        let body = r#"{"error":{"message":"nope"}}"#;
        let (base, _rx) = serve(vec![(status, body.into()), (status, body.into())]);
        let (code, _, err) = run(&base, &["credits", "--json"], None);
        assert_eq!(code, want, "balance HTTP {status}: {err}");
        let (code, _, err) = run(&base, &["credits", "history", "--json"], None);
        assert_eq!(code, want, "history HTTP {status}: {err}");
    }
}
