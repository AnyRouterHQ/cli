use std::collections::BTreeMap;

#[cfg(feature = "native")]
use crate::VERSION;

#[cfg(feature = "native")]
fn agent() -> ureq::Agent {
    use std::sync::OnceLock;
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent(&format!("anyr-cli/{VERSION}"))
                .build()
        })
        .clone()
}

/// Join a CLI profile `base_url` (`https://anyrouter.dev/api`, no `/v1`) with
/// an SDK path. JS `createClient` appends `/v1`, so credits is
/// `https://anyrouter.dev/api/v1/credits`.
pub fn join_api(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{base}{path}")
}

#[cfg(feature = "native")]
fn with_auth(mut req: ureq::Request, api_key: Option<&str>) -> ureq::Request {
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        req = req.set("Authorization", &format!("Bearer {key}"));
    }
    req
}

/// `Could not reach <service>: <url>: <reason>` for a request that got no
/// HTTP answer. ureq nests an I/O error inside its own kind, so its text
/// repeats itself ("Network Error: Network Error: …"); name the cause once.
#[cfg(feature = "native")]
fn unreachable(service: &str, err: &ureq::Error) -> String {
    let ureq::Error::Transport(t) = err else {
        return format!("Could not reach {service}: {err}");
    };
    let full = err.to_string();
    // Windows reports a read timeout as WSAETIMEDOUT (10060) with its own wording.
    let reason =
        if full.to_ascii_lowercase().contains("timed out") || full.contains("os error 10060") {
            "timed out".to_string()
        } else {
            match t.kind() {
                ureq::ErrorKind::Dns => "DNS lookup failed".to_string(),
                ureq::ErrorKind::ConnectionFailed => "could not connect".to_string(),
                _ => {
                    let deepest = std::error::Error::source(err)
                        .map(|e| e.to_string())
                        .or_else(|| t.message().map(str::to_string))
                        .unwrap_or_else(|| t.kind().to_string());
                    let kind = format!("{}: ", t.kind());
                    let mut d = deepest.as_str();
                    while let Some(rest) = d.strip_prefix(&kind) {
                        d = rest;
                    }
                    d.to_string()
                }
            }
        };
    match t.url() {
        Some(url) => format!("Could not reach {service}: {url}: {reason}"),
        None => format!("Could not reach {service}: {reason}"),
    }
}

#[cfg(feature = "native")]
fn into_status_body(result: Result<ureq::Response, ureq::Error>) -> Result<(u16, String), String> {
    match result {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.into_string().unwrap_or_default();
            Ok((status, body))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Ok((code, body))
        }
        Err(err) => Err(unreachable("AnyRouter", &err)),
    }
}

#[cfg(feature = "native")]
pub fn http_get(url: &str, api_key: Option<&str>) -> Result<(u16, String), String> {
    into_status_body(with_auth(agent().get(url), api_key).call())
}

/// GET with its own overall timeout, for optional lookups a launch must not wait on.
#[cfg(feature = "native")]
pub fn http_get_timeout(
    url: &str,
    api_key: Option<&str>,
    timeout: std::time::Duration,
) -> Result<(u16, String), String> {
    into_status_body(with_auth(agent().get(url).timeout(timeout), api_key).call())
}

#[cfg(feature = "native")]
fn into_status_body_labeled(
    result: Result<ureq::Response, ureq::Error>,
    service: &str,
) -> Result<(u16, String), String> {
    match result {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.into_string().unwrap_or_default();
            Ok((status, body))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Ok((code, body))
        }
        Err(err) => Err(unreachable(service, &err)),
    }
}

/// Authenticated GitHub REST GET. `token` must be non-empty — never call
/// `api.github.com` without it (unauth quota returns 403 from shared IPs).
#[cfg(feature = "native")]
pub fn http_get_github(url: &str, token: &str) -> Result<(u16, String), String> {
    let req = agent()
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .set("Authorization", &format!("Bearer {token}"));
    into_status_body_labeled(req.call(), "GitHub Releases")
}

/// Unauthenticated GitHub REST GET for public repos. Works from home
/// networks (60 req/hour); shared IPs (CI) may get 403 and should use
/// `http_get_github` with a token instead.
#[cfg(feature = "native")]
pub fn http_get_github_unauth(url: &str) -> Result<(u16, String), String> {
    let req = agent()
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .set("User-Agent", &format!("anyr-cli/{VERSION}"));
    into_status_body_labeled(req.call(), "GitHub Releases")
}

/// `github.com` HTML (releases listing / expanded_assets). Not the REST API.
#[cfg(feature = "native")]
pub fn http_get_web(url: &str) -> Result<(u16, String), String> {
    into_status_body_labeled(agent().get(url).call(), "GitHub")
}

#[cfg(feature = "native")]
pub fn http_post(
    url: &str,
    api_key: Option<&str>,
    json_body: Option<&str>,
) -> Result<(u16, String), String> {
    let req = with_auth(agent().post(url), api_key).set("Content-Type", "application/json");
    into_status_body(req.send_string(json_body.unwrap_or("{}")))
}

/// POST JSON and hand back the status plus the unread body, for SSE streams.
/// No overall timeout: a long answer must not be cut off at 30s.
#[cfg(feature = "native")]
pub fn http_post_stream(
    url: &str,
    api_key: &str,
    json_body: &str,
) -> Result<(u16, Box<dyn std::io::Read + Send + Sync>), String> {
    http_post_stream_headers(url, api_key, json_body).map(|(status, _, reader)| (status, reader))
}

/// `http_post_stream` that also returns the response headers (lowercased names).
#[cfg(feature = "native")]
pub fn http_post_stream_headers(
    url: &str,
    api_key: &str,
    json_body: &str,
) -> Result<StreamReply, String> {
    let stream_agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        // Per read, so a reader thread left behind by a cancel gives up.
        .timeout_read(std::time::Duration::from_secs(300))
        .user_agent(&format!("anyr-cli/{VERSION}"))
        .build();
    let req = with_auth(stream_agent.post(url), Some(api_key))
        .set("Content-Type", "application/json")
        .set("Accept", "text/event-stream");
    let resp = match req.send_string(json_body) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(_, resp)) => resp,
        Err(err) => return Err(unreachable("AnyRouter", &err)),
    };
    let status = resp.status();
    let headers = resp
        .headers_names()
        .into_iter()
        .filter_map(|n| {
            resp.header(&n)
                .map(|v| (n.to_ascii_lowercase(), v.to_string()))
        })
        .collect();
    Ok((status, headers, resp.into_reader()))
}

/// Status, headers and the unread body of a streamed reply.
#[cfg(feature = "native")]
pub type StreamReply = (
    u16,
    Vec<(String, String)>,
    Box<dyn std::io::Read + Send + Sync>,
);

/// `HTTP <status>: <server message>` for a failed API call.
pub fn error_message(status: u16, body: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let msg = v
        .pointer("/error/message")
        .or_else(|| v.get("error"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            body.split_whitespace()
                .take(30)
                .collect::<Vec<_>>()
                .join(" ")
        });
    format!("HTTP {status}: {msg}")
}

/// `error_message` tagged with the exit code its status maps to (401/403 → 4,
/// 404 → 3, 429 → 5). Use it for an error a command returns; keep
/// `error_message` for text shown inside other output.
pub fn status_error(status: u16, body: &str) -> String {
    crate::exit::tag(crate::exit::for_status(status), error_message(status, body))
}

/// Status, lowercased response headers, body.
pub type HttpReply = (u16, Vec<(String, String)>, String);

/// Non-streaming JSON POST that also returns the response headers (names
/// lowercased), for callers that read `x-anyrouter-*`. 120s total timeout.
#[cfg(feature = "native")]
pub fn http_post_json_headers(
    url: &str,
    api_key: &str,
    json_body: &str,
) -> Result<HttpReply, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent(&format!("anyr-cli/{VERSION}"))
        .build();
    let req = with_auth(agent.post(url), Some(api_key)).set("Content-Type", "application/json");
    let resp = match req.send_string(json_body) {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(err) => return Err(unreachable("AnyRouter", &err)),
    };
    let status = resp.status();
    let headers = resp
        .headers_names()
        .into_iter()
        .filter_map(|n| {
            resp.header(&n)
                .map(|v| (n.to_ascii_lowercase(), v.to_string()))
        })
        .collect();
    Ok((status, headers, resp.into_string().unwrap_or_default()))
}

/// Any method with extra request headers (`anyr api -H k:v`, Idempotency-Key).
#[cfg(feature = "native")]
pub fn http_send(
    method: &str,
    url: &str,
    api_key: Option<&str>,
    headers: &[(String, String)],
    body: Option<&str>,
) -> Result<(u16, String), String> {
    let mut req = with_auth(agent().request(method, url), api_key);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match body {
        Some(b) => into_status_body(req.set("Content-Type", "application/json").send_string(b)),
        None => into_status_body(req.call()),
    }
}

#[cfg(feature = "native")]
pub fn http_delete(url: &str, api_key: Option<&str>) -> Result<(u16, String), String> {
    into_status_body(with_auth(agent().delete(url), api_key).call())
}

#[cfg(feature = "native")]
pub fn http_patch(
    url: &str,
    api_key: Option<&str>,
    json_body: Option<&str>,
) -> Result<(u16, String), String> {
    let req = with_auth(agent().patch(url), api_key).set("Content-Type", "application/json");
    into_status_body(req.send_string(json_body.unwrap_or("{}")))
}

/// PUT a local file as the raw request body, buffered from disk.
#[cfg(feature = "native")]
pub fn http_put_file(
    url: &str,
    api_key: Option<&str>,
    path: &std::path::Path,
) -> Result<(u16, String), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let len = file
        .metadata()
        .map_err(|e| format!("Could not stat {}: {e}", path.display()))?
        .len();
    // Buffer first, then check the size (#223). A file that shrinks or grows
    // mid-upload must fail here as "changed while publishing" instead of
    // sending a short body and waiting out the server's timeout.
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut body)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if body.len() as u64 != len {
        return Err(format!("File {} changed while publishing", path.display()));
    }
    let len = body.len();
    let req = with_auth(agent().put(url), api_key)
        .set("Content-Type", "application/octet-stream")
        .set("Content-Length", &len.to_string());
    into_status_body(req.send(std::io::Cursor::new(body)))
}

/// POST the native Decisions envelope and return the structured response.
/// The endpoint is intentionally `/v1/decisions`; `/v1/systemone` is an API
/// alias, not a chat-completions fallback.
pub fn create_decision(
    base_url: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let url = join_api(base_url, "/v1/decisions");
    let json = serde_json::to_string(body)
        .map_err(|err| format!("Could not encode decision request: {err}"))?;
    let (status, response) = http_post(&url, Some(api_key), Some(&json))?;
    if !(200..300).contains(&status) {
        return Err(decision_error(status, &response));
    }
    serde_json::from_str(&response).map_err(|err| format!("Invalid decision response: {err}"))
}

/// The server's `error.message` says what to fix (for example a question with
/// no `instructions`); a bare status code does not.
fn decision_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message")?.as_str().map(str::to_string));
    match detail {
        Some(msg) => format!("Decision request failed (HTTP {status}): {msg}"),
        None => format!("Decision request failed (HTTP {status})."),
    }
}

#[cfg(not(feature = "native"))]
fn no_network() -> Result<(u16, String), String> {
    Err("network is not available in the browser demo".into())
}

#[cfg(not(feature = "native"))]
pub fn http_get(_url: &str, _api_key: Option<&str>) -> Result<(u16, String), String> {
    no_network()
}

#[cfg(not(feature = "native"))]
pub fn http_get_timeout(
    _url: &str,
    _api_key: Option<&str>,
    _timeout: std::time::Duration,
) -> Result<(u16, String), String> {
    Err("HTTP is not available in the browser demo".into())
}

#[cfg(not(feature = "native"))]
pub fn http_get_github(_url: &str, _token: &str) -> Result<(u16, String), String> {
    no_network()
}

#[cfg(not(feature = "native"))]
pub fn http_get_github_unauth(_url: &str) -> Result<(u16, String), String> {
    no_network()
}

#[cfg(not(feature = "native"))]
pub fn http_get_web(_url: &str) -> Result<(u16, String), String> {
    no_network()
}

#[cfg(not(feature = "native"))]
pub fn http_post(
    _url: &str,
    _api_key: Option<&str>,
    _json_body: Option<&str>,
) -> Result<(u16, String), String> {
    no_network()
}

#[cfg(not(feature = "native"))]
pub fn http_post_json_headers(
    _url: &str,
    _api_key: &str,
    _json_body: &str,
) -> Result<HttpReply, String> {
    Err("network is not available in the browser demo".into())
}

#[cfg(not(feature = "native"))]
pub fn http_send(
    _method: &str,
    _url: &str,
    _api_key: Option<&str>,
    _headers: &[(String, String)],
    _body: Option<&str>,
) -> Result<(u16, String), String> {
    no_network()
}

#[cfg(not(feature = "native"))]
pub fn http_delete(_url: &str, _api_key: Option<&str>) -> Result<(u16, String), String> {
    no_network()
}

#[cfg(not(feature = "native"))]
pub fn http_patch(
    _url: &str,
    _api_key: Option<&str>,
    _json_body: Option<&str>,
) -> Result<(u16, String), String> {
    no_network()
}

#[cfg(not(feature = "native"))]
pub fn http_put_file(
    _url: &str,
    _api_key: Option<&str>,
    _path: &std::path::Path,
) -> Result<(u16, String), String> {
    no_network()
}

/// GET `{base}/v1/credits`. 401 = rejected. 403 falls back to GET `{base}/v1/models`.
pub fn validate_key(base_url: &str, api_key: &str) -> Result<(), String> {
    let credits_url = join_api(base_url, "/v1/credits");
    let (status, _body) = http_get(&credits_url, Some(api_key))?;
    match status {
        200..=299 => Ok(()),
        401 => Err("AnyRouter key was rejected. Check the key in Dashboard -> Keys.".into()),
        403 => {
            let models_url = join_api(base_url, "/v1/models");
            let (probe, _) = http_get(&models_url, Some(api_key))?;
            if (200..300).contains(&probe) {
                Ok(())
            } else if probe == 401 {
                Err("AnyRouter key was rejected. Check the key in Dashboard -> Keys.".into())
            } else {
                Err("HTTP 403 from GET /credits".into())
            }
        }
        other => Err(format!("Could not validate key (HTTP {other}).")),
    }
}

#[derive(Debug, Clone, Default)]
pub struct CatalogModel {
    pub id: String,
    pub name: Option<String>,
    pub owned_by: Option<String>,
    pub context_length: Option<i64>,
    pub max_completion_tokens: Option<i64>,
    pub capabilities: Vec<String>,
    pub pricing: Option<ModelPricing>,
    /// Unix seconds the model joined the catalog.
    pub created: Option<i64>,
    /// What the gateway says about routing this model for the caller now.
    /// None when the response has no such field (an older gateway).
    pub routable: Option<bool>,
}

/// The `/v1/models` field that says whether the caller can route a model now.
/// A model with `false` here is not offered by any picker or agent model list.
pub const ROUTABLE_FIELD: &str = "can_route";

/// USD per 1M tokens, as the catalog lists it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    /// Cache-read price of an upstream that charges exactly `input`/`output`.
    pub cached_input: Option<f64>,
    pub list_input: Option<f64>,
    pub list_output: Option<f64>,
    /// `free`, `byok` or `credits`.
    pub billing: Option<String>,
}

pub fn fetch_models(base_url: &str, api_key: Option<&str>) -> Result<Vec<CatalogModel>, String> {
    fetch_models_sorted(base_url, api_key, "usage")
}

pub fn fetch_models_sorted(
    base_url: &str,
    api_key: Option<&str>,
    sort: &str,
) -> Result<Vec<CatalogModel>, String> {
    let path = format!("/v1/models?privacy=0&sort={sort}");
    let url = join_api(base_url, &path);
    let (status, body) = http_get(&url, api_key)?;
    if !(200..300).contains(&status) {
        return Err(format!("Could not fetch models (HTTP {status})."));
    }
    parse_models_body(&body)
}

/// `fetch_models` with an overall timeout, for a launch that must not wait long.
#[cfg(feature = "native")]
pub fn fetch_models_timeout(
    base_url: &str,
    api_key: Option<&str>,
    timeout: std::time::Duration,
) -> Result<Vec<CatalogModel>, String> {
    let url = join_api(base_url, "/v1/models?privacy=0&sort=usage");
    let (status, body) = http_get_timeout(&url, api_key, timeout)?;
    if !(200..300).contains(&status) {
        return Err(format!("Could not fetch models (HTTP {status})."));
    }
    parse_models_body(&body)
}

/// First catalog id from a `sort=usage` list (7-day request volume).
pub fn most_used_model_id(models: &[CatalogModel]) -> Option<String> {
    models.iter().find_map(|m| {
        let id = crate::spawn::catalog_model_id(&m.id);
        if id.is_empty() || crate::spawn::is_auto_model(&id) {
            None
        } else {
            Some(id)
        }
    })
}

pub(crate) fn parse_models_body(body: &str) -> Result<Vec<CatalogModel>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid models response: {e}"))?;
    let list = value
        .as_array()
        .cloned()
        .or_else(|| value.get("data").and_then(|v| v.as_array()).cloned())
        .or_else(|| value.get("models").and_then(|v| v.as_array()).cloned())
        .unwrap_or_default();
    Ok(list
        .into_iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            Some(CatalogModel {
                id,
                name: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                owned_by: item
                    .get("owned_by")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                context_length: item.get("context_length").and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_u64().map(|n| n as i64))
                        .or_else(|| v.as_f64().map(|n| n as i64))
                }),
                max_completion_tokens: item
                    .pointer("/top_provider/max_completion_tokens")
                    .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64))),
                capabilities: item
                    .get("capabilities")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| c.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                pricing: parse_pricing(&item),
                created: item.get("created").and_then(|v| v.as_i64()),
                routable: item.get(ROUTABLE_FIELD).and_then(|v| v.as_bool()),
            })
        })
        .collect())
}

fn parse_pricing(item: &serde_json::Value) -> Option<ModelPricing> {
    let p = item.get("pricing")?;
    let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(|n| n.as_f64());
    let input = num(p, "input_per_1m")?;
    let output = num(p, "output_per_1m")?;
    let list = p.get("list_pricing");
    let cached_input = item
        .get("per_upstream")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|u| u.get("pricing"))
        .filter(|u| {
            num(u, "input_per_1m") == Some(input) && num(u, "output_per_1m") == Some(output)
        })
        .filter_map(|u| num(u, "input_cache_read_per_1m"))
        .reduce(f64::min);
    Some(ModelPricing {
        input,
        output,
        cached_input,
        list_input: list.and_then(|l| num(l, "input_per_1m")),
        list_output: list.and_then(|l| num(l, "output_per_1m")),
        billing: p
            .get("billing")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// Artificial Analysis intelligence index by catalog id. Public and optional:
/// the picker shows a dash for every model this misses.
pub fn fetch_intelligence(base_url: &str) -> Result<BTreeMap<String, f64>, String> {
    let url = join_api(base_url, "/v1/analytics/model-benchmarks");
    let (status, body) = http_get_timeout(&url, None, std::time::Duration::from_secs(2))?;
    if !(200..300).contains(&status) {
        return Err(format!("Could not fetch model benchmarks (HTTP {status})."));
    }
    parse_intelligence_body(&body)
}

pub(crate) fn parse_intelligence_body(body: &str) -> Result<BTreeMap<String, f64>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid benchmarks response: {e}"))?;
    Ok(value
        .get("models")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("model_id")?.as_str()?.to_string();
            let score = m.get("intelligence_index")?.as_f64()?;
            Some((id, score))
        })
        .collect())
}

impl CatalogModel {
    /// False only when the gateway says the caller cannot route this model.
    pub fn is_routable(&self) -> bool {
        self.routable != Some(false)
    }

    /// `chat`, `embedding` or `decisions`; launchers only accept `chat`.
    pub fn kind(&self) -> &'static str {
        let has = |c: &str| self.capabilities.iter().any(|x| x == c);
        if has("embedding") {
            "embedding"
        } else if has("systemone") || self.id == "anyrouter/decision" {
            "decisions"
        } else {
            "chat"
        }
    }
}

/// `id\towner\tcontext_length\tpinned\ttype` per model, for pipes.
pub fn format_models_tsv(models: &[CatalogModel], pinned_ids: &[String]) -> String {
    models
        .iter()
        .map(|m| {
            format!(
                "{}\t{}\t{}\t{}\t{}\n",
                m.id,
                m.owned_by.as_deref().unwrap_or(""),
                m.context_length.map(|n| n.to_string()).unwrap_or_default(),
                pinned_ids.iter().any(|id| id == &m.id),
                m.kind()
            )
        })
        .collect()
}

pub fn format_models_list(
    models: &[CatalogModel],
    pinned_ids: &[String],
    pinned_preset: Option<&str>,
    json: bool,
) -> (String, String) {
    let auto_id = pinned_ids.first().cloned();
    if json {
        let payload = serde_json::json!({
            "auto": auto_id,
            "preset": pinned_preset,
            "models": models.iter().map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "name": m.name,
                    "owned_by": m.owned_by,
                    "context_length": m.context_length,
                    "pinned": pinned_ids.iter().any(|id| id == &m.id),
                })
            }).collect::<Vec<_>>(),
        });
        return (
            format!(
                "{}\n",
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into())
            ),
            String::new(),
        );
    }
    let mut out_lines = Vec::new();
    if let Some(auto) = &auto_id {
        out_lines.push(format!(
            "PINNED  {auto}  ← auto · {}",
            pinned_preset.unwrap_or("preset")
        ));
        out_lines.push(String::new());
    }
    let id_w = models
        .iter()
        .map(|m| m.id.chars().count())
        .max()
        .unwrap_or(2)
        .max(2);
    out_lines.push(format!(
        "{:<id_w$}  {:<12}  {:<6}  PIN",
        "ID",
        "OWNER",
        "CTX",
        id_w = id_w
    ));
    for model in models {
        let owner = model.owned_by.as_deref().unwrap_or("—");
        let ctx = fmt_context_length(model.context_length);
        let pin = if pinned_ids.iter().any(|id| id == &model.id) {
            "●"
        } else {
            ""
        };
        let id = if pinned_ids.iter().any(|id| id == &model.id) {
            crate::term::model_id(&model.id)
        } else {
            model.id.clone()
        };
        out_lines.push(format!(
            "{:<id_w$}  {:<12}  {:<6}  {pin}",
            id,
            owner,
            ctx,
            id_w = id_w
        ));
    }
    (format!("{}\n", out_lines.join("\n")), String::new())
}

fn fmt_context_length(n: Option<i64>) -> String {
    match n {
        Some(n) if n >= 1_000_000 => format!("{}m", n / 1_000_000),
        Some(n) if n >= 1_000 => format!("{}k", n / 1_000),
        Some(n) => n.to_string(),
        None => "—".into(),
    }
}

/// Marker in errors from a restricted key; `commands::run` maps it to exit 4.
pub const SCOPE_ERROR: &str = "this key lacks scope";

/// Clear message for a 401/403 on an endpoint the key is not allowed to call.
pub fn scope_error(scope: &str) -> String {
    crate::exit::tag(
        crate::exit::AUTH,
        format!(
            "{SCOPE_ERROR} `{scope}` (restricted key). Run `{} login` for a full-access CLI key.",
            crate::help::invoked_bin()
        ),
    )
}

pub fn fetch_credits(base_url: &str, api_key: &str) -> Result<serde_json::Value, String> {
    let url = join_api(base_url, "/v1/credits");
    let (status, body) = http_get(&url, Some(api_key))?;
    if status == 401 || status == 403 {
        return Err(scope_error("credits"));
    }
    if !(200..300).contains(&status) {
        return Err(format!("Could not fetch usage (HTTP {status})."));
    }
    serde_json::from_str(&body).map_err(|e| format!("Invalid credits response: {e}"))
}

/// Identity from GET `/v1/me` (accepts `sk-ar-` inference keys).
#[derive(Debug, Clone, Default)]
pub struct MeInfo {
    pub email: Option<String>,
    pub name: Option<String>,
    pub username: Option<String>,
    pub balance: Option<f64>,
    /// Plan name (`free`, `payg`, `enterprise`), once the server sends it.
    pub plan: Option<String>,
}

impl MeInfo {
    /// `username · email` style label, falling back through name/email/username.
    pub fn display_label(&self) -> String {
        let handle = self
            .username
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.name.as_deref().filter(|s| !s.is_empty()))
            .or(self.email.as_deref().filter(|s| !s.is_empty()));
        match (handle, self.email.as_deref()) {
            (Some(h), Some(e)) if !e.eq_ignore_ascii_case(h) => format!("{h} · {e}"),
            (Some(h), _) => h.to_string(),
            (None, _) => "—".into(),
        }
    }
}

pub fn parse_me(body: &str) -> Result<MeInfo, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid /me response: {e}"))?;
    Ok(MeInfo {
        email: value
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        name: value
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        username: value
            .get("username")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        balance: value.get("balance").and_then(|v| v.as_f64()),
        plan: value
            .get("plan")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

pub fn fetch_me(base_url: &str, api_key: &str) -> Result<MeInfo, String> {
    let url = join_api(base_url, "/v1/me");
    me_from_response(http_get(&url, Some(api_key))?)
}

/// `fetch_me` with an overall deadline, for views that must not hang offline.
pub fn fetch_me_within(
    base_url: &str,
    api_key: &str,
    timeout: std::time::Duration,
) -> Result<MeInfo, String> {
    let url = join_api(base_url, "/v1/me");
    me_from_response(http_get_timeout(&url, Some(api_key), timeout)?)
}

fn me_from_response((status, body): (u16, String)) -> Result<MeInfo, String> {
    if status == 401 || status == 403 {
        return Err(scope_error("me"));
    }
    if !(200..300).contains(&status) {
        return Err(format!("Could not fetch account info (HTTP {status})."));
    }
    parse_me(&body)
}

/// The calling key as GET `/v1/key` describes it. Only the fields the CLI shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyInfo {
    pub name: Option<String>,
    pub is_free_tier: Option<bool>,
    pub last_used_at: Option<String>,
    /// `None` means the key may call every endpoint.
    pub allowed_endpoints: Option<Vec<String>>,
    /// `None` means the key may call every model.
    pub allowed_models: Option<Vec<String>>,
}

pub fn parse_key_info(body: &str) -> Result<KeyInfo, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid /key response: {e}"))?;
    let data = value.get("data").unwrap_or(&value);
    let text = |k: &str| {
        data.get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let list = |k: &str| {
        data.get(k).and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
    };
    Ok(KeyInfo {
        name: text("name"),
        is_free_tier: data.get("is_free_tier").and_then(|v| v.as_bool()),
        last_used_at: text("last_used_at"),
        allowed_endpoints: list("allowed_endpoints").filter(|a| !a.is_empty()),
        allowed_models: list("allowed_models").filter(|a| !a.is_empty()),
    })
}

/// GET `/v1/key` (the bearer key's own row) with an overall deadline.
pub fn fetch_key_within(
    base_url: &str,
    api_key: &str,
    timeout: std::time::Duration,
) -> Result<KeyInfo, String> {
    let url = join_api(base_url, "/v1/key");
    let (status, body) = http_get_timeout(&url, Some(api_key), timeout)?;
    if status == 401 || status == 403 {
        return Err(scope_error("key"));
    }
    if !(200..300).contains(&status) {
        return Err(format!("Could not fetch key info (HTTP {status})."));
    }
    parse_key_info(&body)
}

pub fn format_usd(n: f64) -> String {
    if !n.is_finite() {
        return "$?".into();
    }
    let abs = n.abs();
    let digits = if abs > 0.0 && abs < 0.01 { 4 } else { 2 };
    format!("${n:.digits$}")
}

pub fn format_usage_report(credits: &serde_json::Value, json: bool) -> String {
    if json {
        return format!(
            "{}\n",
            serde_json::to_string_pretty(credits).unwrap_or_else(|_| "{}".into())
        );
    }
    let mut lines = Vec::new();
    if let Some(balance) = credits.get("balance").and_then(|v| v.as_f64()) {
        lines.push(format!(
            "{}  {}",
            crate::term::dim("Credits remaining"),
            crate::term::ok(&format_usd(balance))
        ));
    } else {
        lines.push(format!(
            "{}  {}",
            crate::term::dim("Credits remaining"),
            crate::term::warn("(unknown)")
        ));
    }
    if let Some(used) = credits
        .get("total_usage")
        .or_else(|| credits.get("lifetime_usage"))
        .or_else(|| credits.get("used"))
        .and_then(|v| v.as_f64())
    {
        lines.push(format!(
            "{}  {}",
            crate::term::dim("Lifetime used     "),
            format_usd(used)
        ));
    }
    format!("{}\n", lines.join("\n"))
}

#[derive(Debug, Clone)]
pub struct RemoteKey {
    pub name: String,
    pub hash: String,
    pub masked: String,
    pub created_at: Option<String>,
    pub last_used_at: Option<String>,
    pub active: bool,
    pub id: String,
    /// How the key was made: `cli:<tool>` for device login, null for the dashboard.
    pub origin: Option<String>,
    pub requests_30d: u64,
}

pub fn parse_key_list(body: &str) -> Result<Vec<RemoteKey>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid keys response: {e}"))?;
    let list = value
        .as_array()
        .cloned()
        .or_else(|| value.get("data").and_then(|v| v.as_array()).cloned())
        .unwrap_or_default();
    Ok(list
        .into_iter()
        .filter_map(|item| {
            let hash = item.get("hash")?.as_str()?.to_string();
            let label = item
                .get("label")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("key_prefix").and_then(|v| v.as_str()))
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if hash.len() >= 8 {
                        format!("…{}", &hash[hash.len().saturating_sub(6)..])
                    } else {
                        "—".into()
                    }
                });
            Some(RemoteKey {
                name: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("(unnamed)")
                    .to_string(),
                hash,
                masked: label,
                created_at: item
                    .get("created_at")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                last_used_at: item
                    .get("last_used_at")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                active: item.get("disabled").and_then(|v| v.as_bool()) != Some(true),
                id: item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                origin: item
                    .get("origin")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                requests_30d: item
                    .get("real_usage_monthly_requests")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
            })
        })
        .collect())
}

pub fn fetch_keys(base_url: &str, credential: &str) -> Result<Vec<RemoteKey>, String> {
    let url = join_api(base_url, "/v1/keys");
    let (status, body) = http_get(&url, Some(credential))?;
    if status == 401 || status == 403 {
        return Err(scope_error("keys"));
    }
    if !(200..300).contains(&status) {
        return Err(format!("Could not list keys (HTTP {status})."));
    }
    parse_key_list(&body)
}

pub fn delete_key(base_url: &str, credential: &str, hash: &str) -> Result<(), String> {
    let url = join_api(base_url, &format!("/v1/keys/{hash}"));
    let (status, _body) = http_delete(&url, Some(credential))?;
    if !(200..300).contains(&status) {
        return Err(format!("Could not revoke key (HTTP {status})."));
    }
    Ok(())
}

/// Is this listed key the one currently in use? The list endpoint returns a
/// mask, never the secret — and masks come in two shapes:
///
///   * label-style `sk-ar-v1-abcd…` — everything before the separator is a
///     literal head of the key; compare with starts_with (≥12 chars so a
///     generic prefix like `sk-ar-v1-` can't match every row).
///   * the server's getKeyPrefix() shape `sk-ar-v1-...x9K2` — head + ASCII
///     dots + the key's LAST 4 characters. The head alone is only 9 chars
///     (below the threshold), but the tail is real key material: match with
///     starts_with(head) && ends_with(tail).
///
/// Treating the whole `head...tail` mask as one prefix never matches anything
/// (the middle chars differ from the real key), which made login decide its
/// own stored key "wasn't the newest" and re-reveal/rotate on every run.
pub fn is_active_key_row(masked: &str, api_key: Option<&str>) -> bool {
    let key = api_key.unwrap_or("");
    if key.is_empty() || masked.is_empty() {
        return false;
    }
    if let Some((head, tail)) = masked.split_once("...") {
        // A clean head+tail mask (no further separators inside the tail).
        if !tail.is_empty() && !tail.contains(['…', '.', '*']) {
            return head.len() + tail.len() >= 12 && key.starts_with(head) && key.ends_with(tail);
        }
    }
    // Label-style: everything before the first separator is the head.
    let head = masked.split(['…', '.', '*']).next().unwrap_or("");
    head.len() >= 12 && key.starts_with(head)
}

/// Newest `created_at` first. Missing timestamps sort last.
pub fn keys_newest_first(mut keys: Vec<RemoteKey>) -> Vec<RemoteKey> {
    keys.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.hash.cmp(&a.hash))
    });
    keys
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_gateway_field_is_can_route() {
        assert_eq!(ROUTABLE_FIELD, "can_route");
        let rows = parse_models_body(
            r#"{"data":[{"id":"a","can_route":false,"can_route_reason":"needs_key"},{"id":"b","can_route":true}]}"#,
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|m| m.is_routable()).collect::<Vec<_>>(),
            vec![false, true]
        );
    }

    #[test]
    fn can_route_false_marks_a_model_not_routable_and_absent_means_routable() {
        let body = format!(
            r#"{{"data":[{{"id":"a","{f}":false}},{{"id":"b","{f}":true}},{{"id":"c"}}]}}"#,
            f = ROUTABLE_FIELD
        );
        let rows = parse_models_body(&body).unwrap();
        assert_eq!(
            rows.iter().map(|m| m.is_routable()).collect::<Vec<_>>(),
            vec![false, true, true]
        );
    }

    #[test]
    fn error_message_prefers_the_server_message() {
        assert_eq!(
            error_message(402, r#"{"error":{"message":"no credits"}}"#),
            "HTTP 402: no credits"
        );
    }

    #[test]
    fn models_tsv_has_type_column_chat_embedding_decisions() {
        let body = r#"{"data":[
            {"id":"a/chat","capabilities":["chat"]},
            {"id":"n/embed","capabilities":["embedding"]},
            {"id":"anyrouter/decision","capabilities":["systemone"]},
            {"id":"b/nocaps"}]}"#;
        let models = super::parse_models_body(body).unwrap();
        let tsv = super::format_models_tsv(&models, &[]);
        let types: Vec<&str> = tsv.lines().map(|l| l.split('\t').nth(4).unwrap()).collect();
        assert_eq!(types, ["chat", "embedding", "decisions", "chat"]);
    }

    #[test]
    fn decision_error_shows_the_server_message() {
        let body = r#"{"error":{"code":"invalid_questions","message":"Question \"urgent\" requires instructions as a string, object, or array."}}"#;
        assert_eq!(
            super::decision_error(400, body),
            "Decision request failed (HTTP 400): Question \"urgent\" requires instructions as a string, object, or array."
        );
        assert_eq!(
            super::decision_error(502, "<html>bad gateway</html>"),
            "Decision request failed (HTTP 502)."
        );
    }

    use super::*;
    use crate::config::DEFAULT_BASE_URL;

    #[cfg(feature = "native")]
    #[test]
    fn a_server_that_never_answers_reads_timed_out_once() {
        // WHY: ureq nests its I/O error in its own kind, and the status row
        // printed "Network Error: Network Error: Error encountered in the
        // status line: timed out reading response".
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/v1/key", listener.local_addr().unwrap());
        let hold = std::thread::spawn(move || listener.accept().map(|(s, _)| s));
        let err = http_get_timeout(&url, None, std::time::Duration::from_millis(300)).unwrap_err();
        assert_eq!(err, format!("Could not reach AnyRouter: {url}: timed out"));
        drop(hold.join());
    }

    #[cfg(feature = "native")]
    #[test]
    fn a_refused_connection_says_so_without_repeating_itself() {
        let err = http_get("http://127.0.0.1:9/api/v1/key", None).unwrap_err();
        assert!(err.starts_with("Could not reach AnyRouter: "), "{err}");
        assert!(!err.contains("Network Error: Network Error"), "{err}");
        assert!(err.ends_with("could not connect"), "{err}");
    }

    #[test]
    fn me_reads_the_plan_when_the_server_sends_it() {
        let me = parse_me(r#"{"email":"a@b.c","balance":1,"plan":"payg"}"#).unwrap();
        assert_eq!(me.plan.as_deref(), Some("payg"));
        let me = parse_me(r#"{"email":"a@b.c","plan":""}"#).unwrap();
        assert_eq!(me.plan, None);
    }

    #[test]
    fn catalog_pricing_takes_cached_input_only_from_a_matching_upstream() {
        // WHY: a cache price from a pricier upstream is a price no route
        // at the headline rate charges; the picker must show a dash instead.
        let body = r#"{"data":[
          {"id":"a/x","created":1790000000,"pricing":{"input_per_1m":4,"output_per_1m":20,
            "billing":"byok","list_pricing":{"input_per_1m":5,"output_per_1m":20}},
           "per_upstream":[
            {"pricing":{"input_per_1m":5,"output_per_1m":25,"input_cache_read_per_1m":0.5}},
            {"pricing":{"input_per_1m":4,"output_per_1m":20,"input_cache_read_per_1m":0.4}}]},
          {"id":"b/y","pricing":{"input_per_1m":1,"output_per_1m":2},
           "per_upstream":[{"pricing":{"input_per_1m":3,"output_per_1m":4,"input_cache_read_per_1m":0.3}}]},
          {"id":"c/z"}
        ]}"#;
        let models = parse_models_body(body).unwrap();
        let p = models[0].pricing.clone().unwrap();
        assert_eq!((p.input, p.output, p.cached_input), (4.0, 20.0, Some(0.4)));
        assert_eq!(
            (p.list_input, p.billing.as_deref()),
            (Some(5.0), Some("byok"))
        );
        assert_eq!(models[0].created, Some(1_790_000_000));
        assert_eq!(models[1].pricing.as_ref().unwrap().cached_input, None);
        assert_eq!(models[2].pricing, None);
    }

    #[test]
    fn intelligence_skips_entries_without_a_score() {
        let body = r#"{"models":[
          {"model_id":"a/x","intelligence_index":59.9},
          {"model_id":"b/y","output_tokens_per_second":0}]}"#;
        let scores = parse_intelligence_body(body).unwrap();
        assert_eq!(scores.len(), 1);
        assert_eq!(scores.get("a/x"), Some(&59.9));
    }

    #[test]
    fn join_api_appends_v1_like_js_create_client() {
        assert_eq!(
            join_api(DEFAULT_BASE_URL, "/v1/credits"),
            "https://anyrouter.dev/api/v1/credits"
        );
        assert_eq!(
            join_api(DEFAULT_BASE_URL, "/v1/models"),
            "https://anyrouter.dev/api/v1/models"
        );
        assert_eq!(
            join_api("http://127.0.0.1:9/api", "/v1/credits"),
            "http://127.0.0.1:9/api/v1/credits"
        );
    }

    #[test]
    fn parse_key_list_accepts_data_wrapper() {
        let keys = parse_key_list(
            r#"{"data":[{"id":"key_1","hash":"abc123def456","name":"laptop","key_prefix":"sk-ar-v1-abcd","origin":"cli:cli","real_usage_monthly_requests":12}]}"#,
        )
        .unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "laptop");
        assert_eq!(keys[0].masked, "sk-ar-v1-abcd");
        assert_eq!(keys[0].id, "key_1");
        assert_eq!(keys[0].origin.as_deref(), Some("cli:cli"));
        assert_eq!(keys[0].requests_30d, 12);
    }

    #[test]
    fn parse_me_reads_identity_and_balance() {
        let me = parse_me(
            r#"{"id":"user_1","email":"a@b.co","name":"duyet","image_url":"https://x/y.png","username":"duyet","balance":129.22}"#,
        )
        .unwrap();
        assert_eq!(me.email.as_deref(), Some("a@b.co"));
        assert_eq!(me.username.as_deref(), Some("duyet"));
        assert_eq!(me.balance, Some(129.22));
        assert_eq!(me.display_label(), "duyet · a@b.co");
    }

    #[test]
    fn format_models_list_is_a_table() {
        let models = vec![
            CatalogModel {
                id: "anthropic/claude-sonnet-4.6".into(),
                name: Some("Claude Sonnet 4.6".into()),
                owned_by: Some("anthropic".into()),
                context_length: Some(200_000),
                max_completion_tokens: None,
                ..Default::default()
            },
            CatalogModel {
                id: "openai/gpt-5.4-mini".into(),
                name: None,
                owned_by: Some("openai".into()),
                context_length: Some(128_000),
                max_completion_tokens: None,
                ..Default::default()
            },
        ];
        let pinned = vec!["anthropic/claude-sonnet-4.6".into()];
        let (out, _) = format_models_list(&models, &pinned, Some("@preset/coding-stack"), false);
        assert!(out.contains("PINNED  anthropic/claude-sonnet-4.6"), "{out}");
        assert!(out.contains("ID"), "{out}");
        assert!(out.contains("OWNER"), "{out}");
        assert!(out.contains("200k"), "{out}");
        assert!(out.contains("128k"), "{out}");
        assert!(out.contains('●'), "{out}");
        assert!(!out.contains("auto ->"), "{out}");
    }

    #[test]
    fn parse_models_reads_context_and_most_used_is_first() {
        let models = parse_models_body(
            r#"{"data":[
                {"id":"stealth/ox-alpha","context_length":1000000},
                {"id":"openai/gpt-5.4-mini","context_length":128000}
            ]}"#,
        )
        .unwrap();
        assert_eq!(models[0].id, "stealth/ox-alpha");
        assert_eq!(models[0].context_length, Some(1_000_000));
        assert_eq!(
            most_used_model_id(&models).as_deref(),
            Some("stealth/ox-alpha")
        );
    }

    #[test]
    fn keys_newest_first_orders_by_created_at() {
        let keys = vec![
            RemoteKey {
                name: "old".into(),
                hash: "h1".into(),
                masked: "sk-ar-v1-aaaa".into(),
                created_at: Some("2026-01-01T00:00:00Z".into()),
                last_used_at: None,
                active: true,
                id: String::new(),
                origin: None,
                requests_30d: 0,
            },
            RemoteKey {
                name: "new".into(),
                hash: "h2".into(),
                masked: "sk-ar-v1-bbbb".into(),
                created_at: Some("2026-08-24T12:00:00Z".into()),
                last_used_at: None,
                active: true,
                id: String::new(),
                origin: None,
                requests_30d: 0,
            },
            RemoteKey {
                name: "undated".into(),
                hash: "h0".into(),
                masked: "sk-ar-v1-cccc".into(),
                created_at: None,
                last_used_at: None,
                active: true,
                id: String::new(),
                origin: None,
                requests_30d: 0,
            },
        ];
        let sorted = keys_newest_first(keys);
        assert_eq!(
            sorted.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(),
            vec!["new", "old", "undated"]
        );
    }

    #[test]
    fn key_info_reads_the_data_envelope_and_treats_empty_lists_as_unrestricted() {
        let k = parse_key_info(
            r#"{"data":{"name":"cli","is_free_tier":true,"last_used_at":"2026-10-27T15:31:41Z","allowed_endpoints":[],"allowed_models":["a/b"],"hash":"x"}}"#,
        )
        .unwrap();
        assert_eq!(k.name.as_deref(), Some("cli"));
        assert_eq!(k.is_free_tier, Some(true));
        assert_eq!(k.allowed_endpoints, None);
        assert_eq!(k.allowed_models, Some(vec!["a/b".to_string()]));
        assert!(parse_key_info("nope").is_err());
    }

    #[test]
    fn me_display_label_falls_back_gracefully() {
        let mut me = MeInfo::default();
        assert_eq!(me.display_label(), "—");
        me.email = Some("solo@b.co".into());
        assert_eq!(me.display_label(), "solo@b.co");
        me.name = Some("Solo".into());
        assert_eq!(me.display_label(), "Solo · solo@b.co");
        // Email used as handle should not repeat.
        me.username = Some("solo@b.co".into());
        assert_eq!(me.display_label(), "solo@b.co");
    }

    #[test]
    fn is_active_key_row_matches_prefix_only() {
        assert!(is_active_key_row(
            "sk-ar-v1-abcd…zzzz",
            Some("sk-ar-v1-abcd-secret")
        ));
        assert!(!is_active_key_row(
            "sk-ar-v1-abcd…zzzz",
            Some("sk-ar-v1-zzzz")
        ));
        assert!(!is_active_key_row("short", Some("sk-ar-v1-abcd-secret")));
    }

    #[test]
    fn parse_models_reads_context_and_max_completion_tokens() {
        let body = r#"{"data":[{"id":"minimax/minimax-m3.1-flash","context_length":1000000,"top_provider":{"context_length":1000000,"max_completion_tokens":524288}}]}"#;
        let m = parse_models_body(body).unwrap();
        assert_eq!(m[0].context_length, Some(1_000_000));
        assert_eq!(m[0].max_completion_tokens, Some(524_288));
    }

    #[test]
    fn is_active_key_row_matches_server_getkeyprefix_mask() {
        // Server getKeyPrefix(): first 9 chars + "..." + last 4. The tail is
        // NOT part of the key's head — matching on the whole mask never
        // succeeds and made every login think its stored key was a different
        // one (relogin / re-reveal loop).
        let mask = "sk-ar-v1-...x9K2";
        assert!(is_active_key_row(
            mask,
            Some("sk-ar-v1-Ab3dF7hJ9kLmNpQrStUvWxYz0123456789x9K2")
        ));
        // A different key with the same head must NOT match.
        assert!(!is_active_key_row(
            mask,
            Some("sk-ar-v1-DifferentTail00000000000000000000abcd")
        ));
        // A longer-head ASCII-dot mask matches on head AND literal tail.
        let long = get_key_prefix_style_mask("sk-ar-v1-abcd", "wxyz");
        assert!(is_active_key_row(
            &long,
            Some("sk-ar-v1-abcd-middle-secret-wxyz")
        ));
        assert!(!is_active_key_row(&long, Some("sk-ar-v1-abcd-other-tail")));
    }

    /// Helper mirroring the server's getKeyPrefix() shape for tests.
    fn get_key_prefix_style_mask(head: &str, tail: &str) -> String {
        format!("{head}...{tail}")
    }
}
