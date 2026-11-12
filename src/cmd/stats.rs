//! `anyr stats [report]`: read-only analytics. One table drives every report;
//! `--json` prints the server's JSON unchanged.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::api::{context, get, management_ctx, query};
use crate::cmd::dispatch::USAGE;
use crate::cmd::view::{print_json, print_table, text};
use crate::parse::{get_string_flag, ParsedArgs};

/// A report: its route, the field holding its rows ("" = the body is one
/// row), the columns shown, and the flags the route reads with their allowed
/// values (empty = free text, checked by the server; `#` = a whole number).
struct Report {
    name: &'static str,
    path: &'static str,
    rows: &'static str,
    columns: &'static [&'static str],
    flags: &'static [(&'static str, &'static [&'static str])],
    /// Your workspace's data, read with a management key (read:analytics).
    /// Otherwise platform-wide public data that needs no key.
    private: bool,
}

const NUMBER: &[&str] = &["#"];

const REPORTS: &[Report] = &[
    Report {
        name: "summary",
        path: "analytics/summary",
        rows: "",
        columns: &[
            "total_requests",
            "total_tokens",
            "total_cost",
            "avg_latency_ms",
            "cache_hit_rate",
        ],
        flags: &[("period", &["7d", "30d", "90d", "180d", "365d"])],
        private: true,
    },
    Report {
        name: "usage",
        path: "analytics/usage-range",
        rows: "daily",
        columns: &[
            "date",
            "requests",
            "input_tokens",
            "output_tokens",
            "cost_usd",
        ],
        flags: &[("from", &[]), ("to", &[])],
        private: true,
    },
    Report {
        name: "keys",
        path: "analytics/keys-usage",
        rows: "data",
        columns: &[
            "name",
            "prefix",
            "requests",
            "input_tokens",
            "output_tokens",
            "cost_usd",
            "last_used_at",
        ],
        flags: &[("days", NUMBER)],
        private: true,
    },
    Report {
        name: "models",
        path: "analytics/top-models",
        rows: "models",
        columns: &[
            "rank",
            "model_id",
            "requests",
            "tokens",
            "cost",
            "success_rate",
        ],
        flags: &[
            (
                "period",
                &["this_week", "this_month", "this_quarter", "this_year"],
            ),
            ("limit", NUMBER),
        ],
        private: false,
    },
    Report {
        name: "burn",
        path: "analytics/token-burn",
        rows: "points",
        columns: &["date", "tokens", "cumulative"],
        flags: &[("range", &["3m", "12m", "all"])],
        private: false,
    },
];

/// Report names, in help order.
pub(crate) const SUBCOMMANDS: &[&str] = &["summary", "usage", "keys", "models", "burn"];

/// Flags that pick what a report shows; each report accepts only its own.
const REPORT_FLAGS: &[&str] = &["period", "from", "to", "days", "limit", "range"];

pub(crate) fn run_stats(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
) -> Result<i32, String> {
    let name = parsed.passthrough.first().map_or("summary", String::as_str);
    let report = REPORTS
        .iter()
        .find(|r| r.name == name)
        .filter(|_| parsed.passthrough.len() <= 1)
        .ok_or_else(|| {
            crate::cmd::dispatch::hint(&format!(
                "Unknown stats report \"{}\". Try: {}",
                parsed.passthrough.join(" "),
                SUBCOMMANDS.join(" | ")
            ))
        })?;
    check_flags(parsed, report)?;
    let ctx = context(parsed, env);
    let names: Vec<&str> = report.flags.iter().map(|(f, _)| *f).collect();
    let path = format!("/v1/{}{}", report.path, query(parsed, &names));
    let body = if report.private {
        let mctx = management_ctx(&ctx, "stats", "read:analytics")?;
        get(&mctx, &path, true)?
    } else {
        get(&ctx, &path, false)?
    };
    if ctx.json {
        print_json(&body);
        return Ok(0);
    }
    let rows: Vec<Value> = if report.rows.is_empty() {
        vec![body]
    } else {
        body.get(report.rows)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    if rows.is_empty() {
        eprintln!("(none)");
        return Ok(0);
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|r| report.columns.iter().map(|c| text(r.get(*c))).collect())
        .collect();
    print_table(report.columns, &table);
    Ok(0)
}

/// A flag this report does not read, or a value outside its choices, is a
/// usage error: the server would quietly fall back to its default instead.
fn check_flags(parsed: &ParsedArgs, report: &Report) -> Result<(), String> {
    for flag in REPORT_FLAGS {
        let Some(value) = get_string_flag(&parsed.flags, flag) else {
            continue;
        };
        let Some((_, choices)) = report.flags.iter().find(|(f, _)| f == flag) else {
            return Err(format!(
                "{USAGE}stats {} does not take --{flag}.",
                report.name
            ));
        };
        let ok = match *choices {
            [] => true,
            ["#"] => value.parse::<u32>().is_ok_and(|n| n > 0),
            _ => choices.contains(&value.as_str()),
        };
        if !ok {
            let want = match *choices {
                ["#"] => "a whole number above 0".to_string(),
                _ => format!("one of {}", choices.join(", ")),
            };
            return Err(format!("{USAGE}--{flag} must be {want}."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_report_is_a_subcommand_and_reads_only_known_flags() {
        // WHY: help, completion and schema list SUBCOMMANDS; a report missing
        // there would be unreachable for an agent reading the schema.
        assert_eq!(
            REPORTS.iter().map(|r| r.name).collect::<Vec<_>>(),
            SUBCOMMANDS
        );
        for r in REPORTS {
            for (f, _) in r.flags {
                assert!(REPORT_FLAGS.contains(f), "{}: --{f}", r.name);
            }
        }
    }
}
