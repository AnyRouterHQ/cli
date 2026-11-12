//! `anyr models --free/--type/--owner/--search/--sort` and `anyr models show <id>`.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::api::{context, get, rows, Ctx};
use crate::cmd::dispatch::{hint, USAGE};
use crate::cmd::view::{print_json, print_table, text};
use crate::parse::{get_string_flag, ParsedArgs};

pub(crate) const FILTER_FLAGS: &[&str] = &["free", "type", "owner", "search", "sort"];

const TYPES: &[&str] = &["chat", "embedding", "decision"];
const SORTS: &[&str] = &["price", "context", "new"];

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Filter {
    pub free: bool,
    pub kind: Option<String>,
    pub owner: Option<String>,
    pub search: Option<String>,
    pub sort: Option<String>,
}

impl Filter {
    pub(crate) fn from_args(parsed: &ParsedArgs) -> Result<Self, String> {
        let one_of = |flag: &str, allowed: &[&str]| -> Result<Option<String>, String> {
            match get_string_flag(&parsed.flags, flag) {
                Some(v) if !allowed.contains(&v.as_str()) => Err(format!(
                    "{USAGE}--{flag} must be one of: {}.",
                    allowed.join(", ")
                )),
                other => Ok(other),
            }
        };
        Ok(Self {
            free: parsed.flag_true("free"),
            kind: one_of("type", TYPES)?,
            owner: get_string_flag(&parsed.flags, "owner"),
            search: get_string_flag(&parsed.flags, "search"),
            sort: one_of("sort", SORTS)?,
        })
    }

    pub(crate) fn is_set(parsed: &ParsedArgs) -> bool {
        FILTER_FLAGS.iter().any(|f| parsed.flags.contains_key(*f))
    }
}

/// `chat`, `embedding` or `decision` (what the catalog calls `systemone`).
pub(crate) fn model_type(m: &Value) -> &'static str {
    let caps = |c: &str| {
        m.get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(c)))
    };
    match m.get("category").and_then(Value::as_str) {
        Some("embedding") => "embedding",
        _ if caps("systemone") || caps("decision") => "decision",
        Some("rerank") => "rerank",
        _ => "chat",
    }
}

fn price(m: &Value, side: &str) -> Option<f64> {
    m.pointer(&format!("/pricing/{side}"))
        .and_then(Value::as_f64)
}

/// Free means an explicit $0 price on a platform route; BYOK list prices never count.
pub(crate) fn is_free(m: &Value) -> bool {
    m.pointer("/pricing/billing").and_then(Value::as_str) != Some("byok")
        && price(m, "input_per_1m") == Some(0.0)
        && price(m, "output_per_1m") == Some(0.0)
}

fn str_of<'a>(m: &'a Value, key: &str) -> &'a str {
    m.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn apply(models: &[Value], f: &Filter) -> Vec<Value> {
    let needle = f.search.as_ref().map(|s| s.to_lowercase());
    let mut out: Vec<Value> = models
        .iter()
        .filter(|m| !f.free || is_free(m))
        .filter(|m| f.kind.as_deref().is_none_or(|k| model_type(m) == k))
        .filter(|m| {
            f.owner.as_ref().is_none_or(|o| {
                let o = o.to_lowercase();
                str_of(m, "id").to_lowercase().starts_with(&format!("{o}/"))
                    || str_of(m, "owned_by").to_lowercase() == o
            })
        })
        .filter(|m| {
            needle.as_ref().is_none_or(|n| {
                str_of(m, "id").to_lowercase().contains(n)
                    || str_of(m, "name").to_lowercase().contains(n)
            })
        })
        .cloned()
        .collect();
    let num = |m: &Value, side: &str| price(m, side).unwrap_or(f64::MAX);
    match f.sort.as_deref() {
        Some("price") => crate::sort::sort_by(&mut out, |a, b| {
            num(a, "input_per_1m")
                .partial_cmp(&num(b, "input_per_1m"))
                .unwrap_or(Ordering::Equal)
                .then(
                    num(a, "output_per_1m")
                        .partial_cmp(&num(b, "output_per_1m"))
                        .unwrap_or(Ordering::Equal),
                )
        }),
        Some("context") => crate::sort::sort_by_key(&mut out, |m| {
            std::cmp::Reverse(m.get("context_length").and_then(Value::as_i64).unwrap_or(0))
        }),
        Some("new") => crate::sort::sort_by_key(&mut out, |m| {
            std::cmp::Reverse(m.get("created").and_then(Value::as_i64).unwrap_or(0))
        }),
        _ => {}
    }
    out
}

fn usd_per_m(n: Option<f64>) -> String {
    n.map_or("-".into(), |v| format!("${v}"))
}

fn context_label(n: Option<i64>) -> String {
    match n {
        None => "-".into(),
        Some(n) if n >= 1_000_000 && n % 1_000_000 == 0 => format!("{}M", n / 1_000_000),
        Some(n) if n >= 1000 => format!("{}K", n / 1000),
        Some(n) => n.to_string(),
    }
}

pub(crate) const HEADERS: &[&str] = &["id", "type", "context", "$/1M in", "$/1M out", "free"];

pub(crate) fn table_rows(models: &[Value]) -> Vec<Vec<String>> {
    models
        .iter()
        .map(|m| {
            vec![
                text(m.get("id")),
                model_type(m).to_string(),
                context_label(m.get("context_length").and_then(Value::as_i64)),
                usd_per_m(price(m, "input_per_1m")),
                usd_per_m(price(m, "output_per_1m")),
                if is_free(m) { "free" } else { "" }.to_string(),
            ]
        })
        .collect()
}

pub(crate) fn run_list(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let filter = Filter::from_args(parsed)?;
    let ctx = context(parsed, env);
    let body = get(&ctx, "/v1/models", false)?;
    let shown = apply(&rows(&body), &filter);
    if ctx.json {
        print_json(&json!({"object": "list", "data": shown}));
    } else if shown.is_empty() {
        eprintln!("(none)");
    } else {
        print_table(HEADERS, &table_rows(&shown));
    }
    Ok(0)
}

pub(crate) fn run_show(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let Some(id) = parsed.passthrough.get(1).filter(|s| !s.is_empty()) else {
        return Err(hint("Usage: {bin} models show <id> [--json]"));
    };
    let ctx = context(parsed, env);
    let model = fetch_one(&ctx, id)?;
    if ctx.json {
        print_json(&model);
    } else {
        print!("{}", render_detail(&model));
    }
    Ok(0)
}

fn fetch_one(ctx: &Ctx, id: &str) -> Result<Value, String> {
    get(
        ctx,
        &format!("/v1/models/{}", crate::api::encode(id)),
        false,
    )
    .map_err(|e| {
        if e.contains("HTTP 404") {
            format!(
                "unknown model \"{id}\"; list ids with `{} models`",
                crate::help::invoked_bin()
            )
        } else {
            e
        }
    })
}

pub(crate) fn render_detail(m: &Value) -> String {
    let dim = crate::term::dim;
    let mut out = String::new();
    let mut line =
        |k: &str, v: String| out.push_str(&format!("{}  {v}\n", dim(&format!("{k:<10}"))));
    line("id", text(m.get("id")));
    line("name", text(m.get("name")));
    line("type", model_type(m).to_string());
    line("owner", text(m.get("owned_by")));
    line(
        "context",
        context_label(m.get("context_length").and_then(Value::as_i64)),
    );
    let modalities = |k: &str| {
        m.pointer(&format!("/architecture/{k}"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    line(
        "modalities",
        format!(
            "{} -> {}",
            modalities("input_modalities"),
            modalities("output_modalities")
        ),
    );
    line(
        "price",
        format!(
            "{} in / {} out per 1M tokens",
            usd_per_m(price(m, "input_per_1m")),
            usd_per_m(price(m, "output_per_1m"))
        ),
    );
    let billing = m.pointer("/pricing/billing").and_then(Value::as_str);
    if billing == Some("byok") {
        line(
            "billing",
            "your own provider key (BYOK); price is the provider list price".into(),
        );
    }
    let free_route = if is_free(m) {
        if m.get("free_route_status").is_some() {
            "free route currently unavailable"
        } else {
            "yes"
        }
    } else {
        "no"
    };
    line("free route", free_route.into());
    if let Some(hops) = m.get("per_upstream").and_then(Value::as_array) {
        out.push_str(&format!("{}\n", dim("providers (in order)")));
        for h in hops {
            let tag = match h.get("billing").and_then(Value::as_str) {
                Some("byok") => " (byok)",
                _ => "",
            };
            out.push_str(&format!(
                "  {}{tag}  {} in / {} out\n",
                text(h.get("backend")),
                usd_per_m(h.pointer("/pricing/input_per_1m").and_then(Value::as_f64)),
                usd_per_m(h.pointer("/pricing/output_per_1m").and_then(Value::as_f64)),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Value> {
        vec![
            json!({"id":"nvidia/free-a","owned_by":"nvidia","name":"Free A","category":"text","capabilities":["chat"],
                   "created":300,"context_length":128000,"pricing":{"input_per_1m":0,"output_per_1m":0,"billing":"free"}}),
            json!({"id":"z-ai/glm-4.7","owned_by":"zhipu ai","name":"GLM","category":"text","capabilities":["chat"],
                   "created":100,"context_length":204800,"pricing":{"input_per_1m":0.14,"output_per_1m":0.28}}),
            json!({"id":"z-ai/byok-zero","owned_by":"zhipu ai","name":"BYOK","category":"text","capabilities":["chat"],
                   "created":200,"context_length":1000000,"pricing":{"input_per_1m":0,"output_per_1m":0,"billing":"byok"}}),
            json!({"id":"baai/bge","owned_by":"baai","name":"BGE","category":"embedding","capabilities":["embedding"],
                   "created":50,"context_length":512,"pricing":{"input_per_1m":0.005,"output_per_1m":0}}),
            json!({"id":"anyrouter/decision","owned_by":"anyrouter","name":"Decision","category":"text","capabilities":["systemone"],
                   "created":10,"context_length":8000,"pricing":{"input_per_1m":0,"output_per_1m":0,"billing":"free"}}),
        ]
    }

    fn ids(v: &[Value]) -> Vec<&str> {
        v.iter().map(|m| str_of(m, "id")).collect()
    }

    #[test]
    fn free_is_explicit_zero_and_never_byok() {
        // WHY: a BYOK model shows $0 platform price but the user still pays the
        // provider, so listing it as free would mislead people shopping for $0.
        let f = Filter {
            free: true,
            ..Default::default()
        };
        assert_eq!(
            ids(&apply(&fixture(), &f)),
            ["nvidia/free-a", "anyrouter/decision"]
        );
    }

    #[test]
    fn type_filter_separates_chat_embedding_decision() {
        let t = |k: &str| Filter {
            kind: Some(k.into()),
            ..Default::default()
        };
        assert_eq!(ids(&apply(&fixture(), &t("embedding"))), ["baai/bge"]);
        assert_eq!(
            ids(&apply(&fixture(), &t("decision"))),
            ["anyrouter/decision"]
        );
        assert_eq!(apply(&fixture(), &t("chat")).len(), 3);
    }

    #[test]
    fn owner_and_search_narrow_by_id_or_name() {
        let o = Filter {
            owner: Some("z-ai".into()),
            ..Default::default()
        };
        assert_eq!(
            ids(&apply(&fixture(), &o)),
            ["z-ai/glm-4.7", "z-ai/byok-zero"]
        );
        let s = Filter {
            search: Some("GLM".into()),
            ..Default::default()
        };
        assert_eq!(ids(&apply(&fixture(), &s)), ["z-ai/glm-4.7"]);
    }

    #[test]
    fn sorts_by_price_context_and_newest() {
        let s = |k: &str| Filter {
            sort: Some(k.into()),
            ..Default::default()
        };
        assert_eq!(apply(&fixture(), &s("price"))[0]["id"], "nvidia/free-a");
        assert_eq!(ids(&apply(&fixture(), &s("context")))[0], "z-ai/byok-zero");
        assert_eq!(ids(&apply(&fixture(), &s("new")))[0], "nvidia/free-a");
    }

    #[test]
    fn table_has_the_documented_columns() {
        let r = table_rows(&fixture()[..2]);
        assert_eq!(r[0], ["nvidia/free-a", "chat", "128K", "$0", "$0", "free"]);
        assert_eq!(r[1], ["z-ai/glm-4.7", "chat", "204K", "$0.14", "$0.28", ""]);
    }

    #[test]
    fn detail_golden() {
        let m = json!({"id":"z-ai/glm-4.7","name":"GLM-4.7","owned_by":"zhipu ai","category":"text","capabilities":["chat"],
            "context_length":204800,
            "architecture":{"input_modalities":["text"],"output_modalities":["text"]},
            "pricing":{"input_per_1m":0.14,"output_per_1m":0.28},
            "per_upstream":[{"backend":"deepinfra","pricing":{"input_per_1m":0.1,"output_per_1m":0.2}},
                            {"backend":"z-ai-byok","billing":"byok","pricing":{"input_per_1m":0,"output_per_1m":0}}]});
        let out = render_detail(&m);
        for want in [
            "id          z-ai/glm-4.7",
            "type        chat",
            "context     204K",
            "modalities  text -> text",
            "price       $0.14 in / $0.28 out per 1M tokens",
            "free route  no",
            "  deepinfra  $0.1 in / $0.2 out",
            "  z-ai-byok (byok)  $0 in / $0 out",
        ] {
            assert!(out.contains(want), "missing {want:?} in:\n{out}");
        }
    }
}
