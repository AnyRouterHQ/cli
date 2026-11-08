//! Device key labels, the unused-key audit rules, and the revoke offer after a
//! re-login. Pure: callers pass the clock, the key rows and the prompt.
//!
//! The rules mirror the dashboard's keys page
//! (`apps/dashboard/src/app/keys/-components/key-audit.ts` in duyet/anyrouter);
//! both suites check the same fixed-date cases.

use std::collections::BTreeMap;

use crate::civil::{parse_utc, ymd};
use crate::http::{is_active_key_row, RemoteKey};

const DAY: i64 = 86_400;
const DEVICE_MARK: &str = " anyr ";
const MAX_HOST_CHARS: usize = 40;

/// The name device login gives its key: `<host> anyr <YYYY-MM-DD>`.
pub(crate) fn device_key_label(host: &str, now: i64) -> String {
    let host: String = host
        .split('.')
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(MAX_HOST_CHARS)
        .collect();
    let host = if host.is_empty() {
        "device".into()
    } else {
        host
    };
    format!("{host}{DEVICE_MARK}{}", ymd(now))
}

/// The host part of a device-login key name, or None for any other name.
pub(crate) fn device_of(name: &str) -> Option<&str> {
    let (host, date) = name.rsplit_once(DEVICE_MARK)?;
    let b = date.as_bytes();
    let is_date = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && date
            .chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    (is_date && !host.trim().is_empty()).then_some(host)
}

/// This machine's host name: the OS call first, then the environment.
pub(crate) fn machine_host(env: &BTreeMap<String, String>) -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer outlives the call and its length is passed in.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    ["HOSTNAME", "COMPUTERNAME", "HOST"]
        .iter()
        .find_map(|k| env.get(*k).map(|v| v.trim()).filter(|v| !v.is_empty()))
        .unwrap_or("device")
        .to_string()
}

/// The last four characters of a key, from the server's `sk-ar-v1-...abcd` label.
pub(crate) fn last4(masked: &str) -> &str {
    let tail = masked
        .rsplit_once("...")
        .or_else(|| masked.rsplit_once('…'))
        .map_or("", |(_, t)| t);
    if tail.len() == 4 {
        tail
    } else {
        ""
    }
}

/// `…abcd`, or the hash head when the label has no tail.
pub(crate) fn short_ref(row: &RemoteKey) -> String {
    match last4(&row.masked) {
        "" => row.hash.chars().take(8).collect(),
        tail => format!("…{tail}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reason {
    /// Never used and older than 7 days.
    NeverUsed { age_days: i64 },
    /// No use in the last 30 days or more.
    Unused { idle_days: i64 },
    /// An older device-login key from a device that has a newer key, and not
    /// used since that newer key was made.
    ReplacedOnDevice { device: String, newer: String },
}

impl Reason {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Reason::NeverUsed { .. } => "never_used",
            Reason::Unused { .. } => "unused_30d",
            Reason::ReplacedOnDevice { .. } => "replaced_on_device",
        }
    }

    pub(crate) fn text(&self) -> String {
        match self {
            Reason::NeverUsed { age_days } => format!("never used, created {age_days} days ago"),
            Reason::Unused { idle_days } => format!("not used for {idle_days} days"),
            Reason::ReplacedOnDevice { device, newer } => {
                format!("older login on {device}; {newer} replaced it")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Suggestion {
    pub row: RemoteKey,
    pub reason: Reason,
}

/// Keys worth revoking. Never the key `current` (the one making this request),
/// never a key with requests in the last 30 days. Suggestions only.
pub(crate) fn suggest_removals(
    rows: &[RemoteKey],
    current: Option<&str>,
    now: i64,
) -> Vec<Suggestion> {
    let ts = |s: &Option<String>| s.as_deref().and_then(parse_utc);
    let mut out = Vec::new();
    for row in rows {
        if is_active_key_row(&row.masked, current) || row.requests_30d > 0 {
            continue;
        }
        let created = ts(&row.created_at);
        let used = ts(&row.last_used_at);
        let reason = match (used, created) {
            (None, Some(c)) if now - c >= 7 * DAY => Some(Reason::NeverUsed {
                age_days: (now - c) / DAY,
            }),
            (Some(u), _) if now - u >= 30 * DAY => Some(Reason::Unused {
                idle_days: (now - u) / DAY,
            }),
            _ => None,
        }
        .or_else(|| replaced_on_device(row, rows, created, used));
        if let Some(reason) = reason {
            out.push(Suggestion {
                row: row.clone(),
                reason,
            });
        }
    }
    out
}

fn replaced_on_device(
    row: &RemoteKey,
    rows: &[RemoteKey],
    created: Option<i64>,
    used: Option<i64>,
) -> Option<Reason> {
    if !row.origin.as_deref().is_some_and(|o| o.starts_with("cli")) {
        return None;
    }
    let device = device_of(&row.name)?;
    let created = created?;
    let newer = rows
        .iter()
        .filter(|o| o.hash != row.hash && o.active && device_of(&o.name) == Some(device))
        .filter_map(|o| Some((o, o.created_at.as_deref().and_then(parse_utc)?)))
        .filter(|(_, oc)| *oc > created)
        // Same host name on two live machines: still in use since then.
        .filter(|(_, oc)| used.is_none_or(|u| u <= *oc))
        .max_by_key(|(_, oc)| *oc)?;
    Some(Reason::ReplacedOnDevice {
        device: device.to_string(),
        newer: short_ref(newer.0),
    })
}

/// The device-login key this profile held before a re-login, when it is still
/// on the server and is not the key just issued.
pub(crate) fn previous_device_key(
    rows: &[RemoteKey],
    old_key: Option<&str>,
    new_key: &str,
) -> Option<RemoteKey> {
    let old_key = old_key.map(str::trim).filter(|k| !k.is_empty())?;
    if old_key == new_key {
        return None;
    }
    rows.iter()
        .find(|r| {
            is_active_key_row(&r.masked, Some(old_key))
                && !is_active_key_row(&r.masked, Some(new_key))
                && r.origin.as_deref().is_some_and(|o| o.starts_with("cli"))
        })
        .cloned()
}

/// What happened to the previous key after a re-login.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RevokeOutcome {
    Revoked,
    Kept,
    Hinted,
}

/// Ask before revoking the previous device key. Interactive runs prompt and
/// revoke only on yes; anything else prints the command and revokes nothing.
pub(crate) fn offer_revoke_previous(
    prev: &RemoteKey,
    interactive: bool,
    confirm: impl FnOnce(&str) -> bool,
    revoke: impl FnOnce(&str) -> Result<(), String>,
    out: &mut dyn std::io::Write,
) -> Result<RevokeOutcome, String> {
    let label = format!("\"{}\" ({})", prev.name, short_ref(prev));
    if !interactive {
        let _ = writeln!(
            out,
            "Previous key from this device is still active: {label}\nRevoke it: {} keys revoke {}",
            crate::help::invoked_bin(),
            revoke_ref(prev)
        );
        return Ok(RevokeOutcome::Hinted);
    }
    if !confirm(&format!(
        "Revoke the previous key from this device, {label}?"
    )) {
        return Ok(RevokeOutcome::Kept);
    }
    revoke(&prev.hash)?;
    let _ = writeln!(out, "Revoked {label}");
    Ok(RevokeOutcome::Revoked)
}

/// The shortest argument `keys revoke` accepts for this row.
pub(crate) fn revoke_ref(row: &RemoteKey) -> String {
    match last4(&row.masked) {
        "" => row.hash.chars().take(12).collect(),
        tail => tail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-27T12:00:00Z, the fixed "now" both rule suites share.
    const NOW: i64 = 1_793_102_400;

    fn key(name: &str, tail: &str, origin: Option<&str>) -> RemoteKey {
        RemoteKey {
            name: name.into(),
            hash: format!("hash-{tail}"),
            masked: format!("sk-ar-v1-...{tail}"),
            created_at: Some("2026-10-01T00:00:00Z".into()),
            last_used_at: Some("2026-10-26T00:00:00Z".into()),
            active: true,
            id: format!("key_{tail}"),
            origin: origin.map(str::to_string),
            requests_30d: 0,
        }
    }

    fn with(mut k: RemoteKey, created: &str, used: Option<&str>, req: u64) -> RemoteKey {
        k.created_at = Some(created.into());
        k.last_used_at = used.map(str::to_string);
        k.requests_30d = req;
        k
    }

    fn reasons(rows: &[RemoteKey], current: Option<&str>) -> Vec<(String, &'static str)> {
        suggest_removals(rows, current, NOW)
            .into_iter()
            .map(|s| (s.row.name, s.reason.code()))
            .collect()
    }

    #[test]
    fn now_constant_is_the_shared_fixture_date() {
        assert_eq!(ymd(NOW), "2026-10-27");
    }

    #[test]
    fn device_label_is_host_anyr_date_and_round_trips() {
        let label = device_key_label("studio.lan", NOW);
        assert_eq!(label, "studio anyr 2026-10-27");
        assert_eq!(device_of(&label), Some("studio"));
        assert_eq!(device_key_label("", NOW), "device anyr 2026-10-27");
        assert!(device_key_label(&"x".repeat(80), NOW).len() <= 60);
        // Legacy and dashboard names carry no device.
        assert_eq!(device_of("AnyRouter CLI"), None);
        assert_eq!(device_of("ci anyr later"), None);
    }

    #[test]
    fn never_used_needs_seven_days() {
        let rows = vec![
            with(
                key("old-unused", "aaaa", None),
                "2026-10-19T12:00:00Z",
                None,
                0,
            ),
            with(
                key("new-unused", "bbbb", None),
                "2026-10-21T12:00:00Z",
                None,
                0,
            ),
        ];
        assert_eq!(
            reasons(&rows, None),
            vec![("old-unused".into(), "never_used")]
        );
    }

    #[test]
    fn idle_needs_thirty_days_and_zero_recent_requests() {
        let rows = vec![
            with(
                key("idle", "aaaa", None),
                "2026-01-01T00:00:00Z",
                Some("2026-09-26T00:00:00Z"),
                0,
            ),
            with(
                key("recent", "bbbb", None),
                "2026-01-01T00:00:00Z",
                Some("2026-09-30T00:00:00Z"),
                0,
            ),
            // A stale last_used_at with requests in the window is still in use.
            with(
                key("busy", "cccc", None),
                "2026-01-01T00:00:00Z",
                Some("2026-09-01T00:00:00Z"),
                40,
            ),
        ];
        assert_eq!(reasons(&rows, None), vec![("idle".into(), "unused_30d")]);
    }

    #[test]
    fn the_current_key_is_never_suggested() {
        let rows = vec![with(
            key("this-machine", "aaaa", None),
            "2026-01-01T00:00:00Z",
            None,
            0,
        )];
        assert_eq!(reasons(&rows, None).len(), 1);
        let current = "sk-ar-v1-secretsecretsecretsecretaaaa";
        assert!(reasons(&rows, Some(current)).is_empty());
    }

    #[test]
    fn older_device_key_is_replaced_by_a_newer_one_on_the_same_device() {
        let cli = Some("cli:cli");
        let rows = vec![
            with(
                key("studio anyr 2026-10-20", "aaaa", cli),
                "2026-10-20T00:00:00Z",
                Some("2026-10-24T00:00:00Z"),
                5,
            ),
            with(
                key("studio anyr 2026-10-25", "bbbb", cli),
                "2026-10-25T00:00:00Z",
                Some("2026-10-27T00:00:00Z"),
                9,
            ),
        ];
        // requests_30d > 0 keeps the older one: it still saw traffic this month.
        assert!(reasons(&rows, None).is_empty());
        let mut rows = rows;
        rows[0].requests_30d = 0;
        assert_eq!(
            reasons(&rows, None),
            vec![("studio anyr 2026-10-20".into(), "replaced_on_device")]
        );
    }

    #[test]
    fn same_host_name_still_in_use_after_the_newer_key_is_kept() {
        let cli = Some("cli:cli");
        let rows = vec![
            with(
                key("macbook anyr 2026-10-20", "aaaa", cli),
                "2026-10-20T00:00:00Z",
                Some("2026-10-26T00:00:00Z"),
                0,
            ),
            with(
                key("macbook anyr 2026-10-25", "bbbb", cli),
                "2026-10-25T00:00:00Z",
                None,
                0,
            ),
        ];
        assert!(reasons(&rows, None).is_empty());
    }

    #[test]
    fn dashboard_keys_and_legacy_cli_names_never_group_by_device() {
        let rows = vec![
            with(
                key("AnyRouter CLI", "aaaa", Some("cli:cli")),
                "2026-10-20T00:00:00Z",
                Some("2026-10-21T00:00:00Z"),
                0,
            ),
            with(
                key("AnyRouter CLI", "bbbb", Some("cli:cli")),
                "2026-10-25T00:00:00Z",
                Some("2026-10-26T00:00:00Z"),
                0,
            ),
            with(
                key("studio anyr 2026-10-20", "cccc", None),
                "2026-10-20T00:00:00Z",
                Some("2026-10-21T00:00:00Z"),
                0,
            ),
            with(
                key("studio anyr 2026-10-25", "dddd", None),
                "2026-10-25T00:00:00Z",
                Some("2026-10-26T00:00:00Z"),
                0,
            ),
        ];
        assert!(reasons(&rows, None).is_empty());
    }

    #[test]
    fn previous_device_key_is_the_old_stored_cli_key_only() {
        let old = "sk-ar-v1-oldoldoldoldoldoldoldoldaaaa";
        let new = "sk-ar-v1-newnewnewnewnewnewnewnewbbbb";
        let rows = vec![
            key("studio anyr 2026-10-20", "aaaa", Some("cli:cli")),
            key("studio anyr 2026-10-27", "bbbb", Some("cli:cli")),
        ];
        let prev = previous_device_key(&rows, Some(old), new).unwrap();
        assert_eq!(prev.hash, "hash-aaaa");
        // Same key back from the consent screen, no old key, or a pasted
        // dashboard key: nothing to offer.
        assert!(previous_device_key(&rows, Some(old), old).is_none());
        assert!(previous_device_key(&rows, None, new).is_none());
        let pasted = vec![key("ci", "aaaa", None)];
        assert!(previous_device_key(&pasted, Some(old), new).is_none());
    }

    #[test]
    fn relogin_revokes_only_after_yes() {
        let prev = key("studio anyr 2026-10-20", "aaaa", Some("cli:cli"));
        for (answer, want) in [(true, RevokeOutcome::Revoked), (false, RevokeOutcome::Kept)] {
            let mut revoked = Vec::new();
            let mut asked = String::new();
            let mut out = Vec::new();
            let got = offer_revoke_previous(
                &prev,
                true,
                |q| {
                    asked = q.to_string();
                    answer
                },
                |h| {
                    revoked.push(h.to_string());
                    Ok(())
                },
                &mut out,
            )
            .unwrap();
            assert_eq!(got, want);
            assert!(asked.contains("…aaaa"), "{asked}");
            assert_eq!(revoked.len(), usize::from(answer));
        }
    }

    #[test]
    fn non_interactive_relogin_prints_a_hint_and_never_revokes() {
        let prev = key("studio anyr 2026-10-20", "aaaa", Some("cli:cli"));
        let mut out = Vec::new();
        let got = offer_revoke_previous(
            &prev,
            false,
            |_| panic!("must not prompt"),
            |_| panic!("must not revoke"),
            &mut out,
        )
        .unwrap();
        assert_eq!(got, RevokeOutcome::Hinted);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("keys revoke aaaa"), "{text}");
        assert!(!text.contains("sk-ar-v1-"), "{text}");
    }

    #[test]
    fn shared_fixture_cases_match_the_dashboard_rules() {
        // WHY #244: one fixture drives both suites so the rules cannot drift.
        let raw = include_str!("../../tests/fixtures/key-audit-cases.json");
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(raw.as_bytes());
        let hex: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex, "6ef07146e0a1e56bdf771dd31d6ff802d3cd14183d6fabfd23d1f56c18645306",
            "fixture changed; re-vendor tests/fixtures/key-audit-cases.json and update the hash"
        );
        let fixture: serde_json::Value = serde_json::from_str(raw).unwrap();
        let now = crate::civil::parse_utc(fixture["now"].as_str().unwrap()).unwrap();
        assert_eq!(now, NOW);
        for case in fixture["cases"].as_array().unwrap() {
            let rows: Vec<RemoteKey> = case["keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|k| {
                    let tail = k["tail"].as_str().unwrap();
                    RemoteKey {
                        name: k["name"].as_str().unwrap().into(),
                        hash: format!("hash_{tail}"),
                        masked: format!("sk-ar-v1-...{tail}"),
                        created_at: Some(k["created_at"].as_str().unwrap().into()),
                        last_used_at: k["last_used_at"].as_str().map(str::to_string),
                        active: !k["disabled"].as_bool().unwrap_or(false),
                        id: format!("key_{tail}"),
                        origin: k["origin"].as_str().map(str::to_string),
                        requests_30d: k["real_usage_monthly_requests"].as_u64().unwrap_or(0),
                    }
                })
                .collect();
            let current_secret = case["currentKeyId"].as_str().map(|id| {
                let tail = id.strip_prefix("key_").unwrap_or(id);
                format!("sk-ar-v1-test-{tail}")
            });
            let current = current_secret.as_deref();
            let got: Vec<(String, String)> = suggest_removals(&rows, current, now)
                .into_iter()
                .map(|s| (s.row.name, s.reason.code().to_string()))
                .collect();
            let want: Vec<(String, String)> = case["expected"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e["name"].as_str().unwrap().to_string(),
                        e["reason"]["code"].as_str().unwrap().to_string(),
                    )
                })
                .collect();
            assert_eq!(got, want, "case {}", case["name"].as_str().unwrap_or("?"));
            // The numeric and device fields must match too, not just the code.
            for s in suggest_removals(&rows, current, now) {
                let exp = case["expected"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"].as_str() == Some(&s.row.name))
                    .unwrap();
                match s.reason {
                    Reason::NeverUsed { age_days } => {
                        assert_eq!(age_days, exp["reason"]["ageDays"].as_i64().unwrap_or(-1))
                    }
                    Reason::Unused { idle_days } => {
                        assert_eq!(idle_days, exp["reason"]["idleDays"].as_i64().unwrap_or(-1))
                    }
                    Reason::ReplacedOnDevice { device, newer } => {
                        assert_eq!(
                            device,
                            exp["reason"]["device"].as_str().unwrap_or(""),
                            "case {}",
                            case["name"].as_str().unwrap_or("?")
                        );
                        // Rust reports the newer key by its last four; the
                        // fixture names it in full. They must be the same key.
                        let want_name = exp["reason"]["newerName"].as_str().unwrap();
                        let want_row = rows.iter().find(|r| r.name == want_name).unwrap();
                        assert_eq!(newer, short_ref(want_row));
                    }
                }
            }
        }
        for d in fixture["deviceOf"].as_array().unwrap() {
            assert_eq!(
                device_of(d["name"].as_str().unwrap()),
                d["device"].as_str(),
                "deviceOf {:?}",
                d["name"]
            );
        }
        for f in fixture["keyLast4"].as_array().unwrap() {
            assert_eq!(
                last4(f["label"].as_str().unwrap()),
                f["last4"].as_str().unwrap_or(""),
                "keyLast4 {:?}",
                f["label"]
            );
        }
    }
}
