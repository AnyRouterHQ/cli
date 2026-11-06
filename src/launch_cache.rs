//! Local copies of what a launch would otherwise ask for every time: the
//! model catalog (`GET /v1/models`) and the opencode major version
//! (`opencode --version`). A warm launch, and the model picker, read both
//! from disk.
//!
//! The catalog is fresh for 5 minutes, the gateway's `max-age`. An older copy,
//! up to a day, is still used and refreshed in the background while the agent
//! runs, the gateway's `stale-while-revalidate`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::http::CatalogModel;

const CATALOG_FILE: &str = "models-cache.json";
const SCORES_FILE: &str = "model-scores.json";
/// Rows written before the picker read prices and dates from the cache lack
/// them, so a row from another version is ignored.
const CATALOG_VERSION: u64 = 2;
#[cfg(feature = "native")]
const VERSIONS_FILE: &str = "tool-versions.json";
const FRESH_SECS: u64 = 300;
const STALE_SECS: u64 = 86_400;
/// A launch with no copy at all waits at most this long for the catalog.
#[cfg(feature = "native")]
const COLD_FETCH: std::time::Duration = std::time::Duration::from_millis(1500);
/// A launch that writes the model list into the agent's own config waits this
/// long on a cold cache: a short list written then stays what the user can pick.
#[cfg(feature = "native")]
const LIST_FETCH: std::time::Duration = std::time::Duration::from_secs(8);

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Age {
    Fresh,
    Stale,
}

/// One row of a cache file on disk next to the config: the catalog for one
/// gateway and key, or the benchmark scores for one gateway.
pub(crate) struct CatalogCache {
    path: PathBuf,
    row: String,
}

impl CatalogCache {
    pub(crate) fn new(dir: &Path, base: &str, key: Option<&str>) -> Self {
        Self {
            path: dir.join(CATALOG_FILE),
            row: crate::mcp_wiring::cache_key(base, key.unwrap_or("")),
        }
    }

    fn scores(dir: &Path, base: &str) -> Self {
        Self {
            path: dir.join(SCORES_FILE),
            row: base.to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn load(&self, now: u64) -> Option<(Vec<CatalogModel>, Age)> {
        let (data, age) = self.load_value(now)?;
        Some((decode_models(data)?, age))
    }

    #[cfg(test)]
    pub(crate) fn save(&self, models: &[CatalogModel], now: u64) {
        self.save_value(encode_models(models), now);
    }

    fn load_value(&self, now: u64) -> Option<(Value, Age)> {
        let rows = read_json(&self.path)?;
        let row = rows.get(&self.row)?;
        let at = row.get("at")?.as_u64()?;
        if row.get("v").and_then(Value::as_u64) != Some(CATALOG_VERSION)
            || at > now
            || now - at >= STALE_SECS
        {
            return None;
        }
        let age = if now - at < FRESH_SECS {
            Age::Fresh
        } else {
            Age::Stale
        };
        Some((row.get("data")?.clone(), age))
    }

    fn save_value(&self, data: Value, now: u64) {
        let mut rows = read_json(&self.path)
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        if let Some(map) = rows.as_object_mut() {
            map.retain(|_, v| {
                v.get("at")
                    .and_then(Value::as_u64)
                    .is_some_and(|at| now.saturating_sub(at) < STALE_SECS)
            });
            map.insert(
                self.row.clone(),
                json!({ "at": now, "v": CATALOG_VERSION, "data": data }),
            );
        }
        write_json(&self.path, &rows);
    }
}

fn encode_models(models: &[CatalogModel]) -> Value {
    Value::Array(models.iter().map(model_row).collect())
}

fn decode_models(data: Value) -> Option<Vec<CatalogModel>> {
    crate::http::parse_models_body(&json!({ "data": data }).to_string()).ok()
}

fn encode_scores(scores: &BTreeMap<String, f64>) -> Value {
    json!(scores)
}

fn decode_scores(data: Value) -> Option<BTreeMap<String, f64>> {
    serde_json::from_value(data).ok()
}

/// The catalog fields a launch and the picker read, in the API's own shape so
/// the gateway's parser reads them back.
fn model_row(m: &CatalogModel) -> Value {
    let mut row = json!({ "id": m.id, "capabilities": m.capabilities });
    if let Some(name) = &m.name {
        row["name"] = json!(name);
    }
    if let Some(owner) = &m.owned_by {
        row["owned_by"] = json!(owner);
    }
    if let Some(ctx) = m.context_length {
        row["context_length"] = json!(ctx);
    }
    if let Some(max) = m.max_completion_tokens {
        row["top_provider"] = json!({ "max_completion_tokens": max });
    }
    if let Some(created) = m.created {
        row["created"] = json!(created);
    }
    if let Some(routable) = m.routable {
        row[crate::http::ROUTABLE_FIELD] = json!(routable);
    }
    if let Some(p) = &m.pricing {
        row["pricing"] = json!({ "input_per_1m": p.input, "output_per_1m": p.output });
        if let Some(billing) = &p.billing {
            row["pricing"]["billing"] = json!(billing);
        }
        if p.list_input.is_some() || p.list_output.is_some() {
            row["pricing"]["list_pricing"] =
                json!({ "input_per_1m": p.list_input, "output_per_1m": p.list_output });
        }
        if let Some(cached) = p.cached_input {
            row["per_upstream"] = json!([{ "pricing": {
                "input_per_1m": p.input,
                "output_per_1m": p.output,
                "input_cache_read_per_1m": cached,
            } }]);
        }
    }
    row
}

/// The catalog for a launch, plus a background refresh to keep alive while
/// the agent runs. A dry run reads only what is cached.
pub(crate) struct LaunchCatalog {
    pub(crate) models: Option<Vec<CatalogModel>>,
    pub(crate) refresh: Refresh,
}

/// A running background refresh. Hold it until the agent exits.
#[cfg(feature = "native")]
pub(crate) type Refresh = Option<std::thread::JoinHandle<()>>;
#[cfg(not(feature = "native"))]
pub(crate) type Refresh = Option<()>;

pub(crate) fn catalog_for_launch(
    dir: Option<&Path>,
    base: &str,
    key: Option<&str>,
    dry_run: bool,
) -> LaunchCatalog {
    catalog_with(dir, base, key, dry_run, now_secs(), &fetch)
}

/// The catalog for a launch that writes the model list into the agent's own
/// config (pi, crush, fx). With no usable copy it waits up to `LIST_FETCH`
/// after calling `wait`, instead of the short cold budget.
pub(crate) fn catalog_for_list(
    dir: Option<&Path>,
    base: &str,
    key: Option<&str>,
    dry_run: bool,
    wait: &dyn Fn(),
) -> LaunchCatalog {
    catalog_for_list_with(dir, base, key, dry_run, now_secs(), &fetch_patient, wait)
}

pub(crate) fn catalog_for_list_with(
    dir: Option<&Path>,
    base: &str,
    key: Option<&str>,
    dry_run: bool,
    now: u64,
    fetch: &'static Fetch,
    wait: &dyn Fn(),
) -> LaunchCatalog {
    let warm = dir.is_some_and(|d| CatalogCache::new(d, base, key).load_value(now).is_some());
    if !warm && !dry_run {
        wait();
    }
    catalog_with(dir, base, key, dry_run, now, fetch)
}

#[cfg(feature = "native")]
fn fetch_patient(base: &str, key: Option<&str>) -> Result<Vec<CatalogModel>, String> {
    crate::http::fetch_models_timeout(base, key, LIST_FETCH)
}

#[cfg(not(feature = "native"))]
fn fetch_patient(base: &str, key: Option<&str>) -> Result<Vec<CatalogModel>, String> {
    crate::http::fetch_models(base, key)
}

#[cfg(feature = "native")]
fn fetch(base: &str, key: Option<&str>) -> Result<Vec<CatalogModel>, String> {
    crate::http::fetch_models_timeout(base, key, COLD_FETCH)
}

#[cfg(not(feature = "native"))]
fn fetch(base: &str, key: Option<&str>) -> Result<Vec<CatalogModel>, String> {
    crate::http::fetch_models(base, key)
}

pub(crate) type Fetch = dyn Fn(&str, Option<&str>) -> Result<Vec<CatalogModel>, String> + Sync;

pub(crate) type ScoresFetch = dyn Fn(&str) -> Result<BTreeMap<String, f64>, String> + Sync;

pub(crate) fn catalog_with(
    dir: Option<&Path>,
    base: &str,
    key: Option<&str>,
    dry_run: bool,
    now: u64,
    fetch: &'static Fetch,
) -> LaunchCatalog {
    let (base_owned, key_owned) = (base.to_string(), key.map(str::to_string));
    let (models, refresh) = cached(
        dir.map(|d| CatalogCache::new(d, base, key)),
        dry_run,
        now,
        move || fetch(&base_owned, key_owned.as_deref()),
        |m: &Vec<CatalogModel>| encode_models(m),
        decode_models,
    );
    LaunchCatalog { models, refresh }
}

/// Benchmark scores for the picker, with the catalog's freshness rules. An
/// empty map when there is no copy and the fetch fails.
pub(crate) fn scores_with(
    dir: Option<&Path>,
    base: &str,
    now: u64,
    fetch: &'static ScoresFetch,
) -> BTreeMap<String, f64> {
    let base_owned = base.to_string();
    cached(
        dir.map(|d| CatalogCache::scores(d, base)),
        false,
        now,
        move || fetch(&base_owned),
        encode_scores,
        decode_scores,
    )
    .0
    .unwrap_or_default()
}

/// A fresh copy is used as is. A stale one is used while `fetch` refreshes it
/// in the background. With no copy, `fetch` runs now, except on a dry run.
fn cached<T: Send + 'static>(
    cache: Option<CatalogCache>,
    dry_run: bool,
    now: u64,
    fetch: impl FnOnce() -> Result<T, String> + Send + 'static,
    encode: fn(&T) -> Value,
    decode: fn(Value) -> Option<T>,
) -> (Option<T>, Refresh) {
    let copy = cache
        .as_ref()
        .and_then(|c| c.load_value(now))
        .and_then(|(data, age)| Some((decode(data)?, age)));
    match (copy, cache) {
        (Some((value, Age::Stale)), Some(cache)) if !dry_run => {
            (Some(value), spawn_refresh(cache, fetch, encode))
        }
        (Some((value, _)), _) => (Some(value), None),
        (None, _) if dry_run => (None, None),
        (None, cache) => {
            let fetched = fetch().ok();
            if let (Some(c), Some(v)) = (&cache, &fetched) {
                c.save_value(encode(v), now);
            }
            (fetched, None)
        }
    }
}

#[cfg(feature = "native")]
fn spawn_refresh<T: 'static>(
    cache: CatalogCache,
    fetch: impl FnOnce() -> Result<T, String> + Send + 'static,
    encode: fn(&T) -> Value,
) -> Refresh {
    std::thread::Builder::new()
        .name("anyr-catalog".into())
        .spawn(move || {
            if let Ok(value) = fetch() {
                cache.save_value(encode(&value), now_secs());
            }
        })
        .ok()
}

#[cfg(not(feature = "native"))]
fn spawn_refresh<T: 'static>(
    _: CatalogCache,
    _: impl FnOnce() -> Result<T, String>,
    _: fn(&T) -> Value,
) -> Refresh {
    None
}

/// `opencode --version` major, cached by binary path, size and mtime so a
/// reinstall or upgrade is probed again.
#[cfg(feature = "native")]
pub(crate) fn opencode_major(dir: Option<&Path>, command: &str) -> Option<u32> {
    let Some(dir) = dir else {
        return crate::spawn::opencode_major_version(command);
    };
    let stamp = binary_stamp(command)?;
    let path = dir.join(VERSIONS_FILE);
    let mut rows = read_json(&path)
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(row) = rows.get(command) {
        if row.get("stamp").and_then(Value::as_str) == Some(stamp.as_str()) {
            return row.get("major").and_then(Value::as_u64).map(|m| m as u32);
        }
    }
    let major = crate::spawn::opencode_major_version(command);
    rows[command] = json!({ "stamp": stamp, "major": major });
    write_json(&path, &rows);
    major
}

#[cfg(not(feature = "native"))]
pub(crate) fn opencode_major(_dir: Option<&Path>, _command: &str) -> Option<u32> {
    None
}

#[cfg(feature = "native")]
fn binary_stamp(command: &str) -> Option<String> {
    let meta = std::fs::metadata(command).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{}:{mtime}", meta.len()))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Write to a temp file unique to this process and call, then rename over
/// the target, so a concurrent reader or writer never sees a torn file.
fn write_json(path: &Path, value: &Value) {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        let _ = crate::config::ensure_private_dir(dir);
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_file_name(format!("{name}.{}.{seq}.tmp", std::process::id()));
    let written = crate::config::open_private(&tmp, true)
        .and_then(|mut f| f.write_all(value.to_string().as_bytes()));
    if written.is_err() || std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://gw.test/api";
    const KEY: &str = "sk-ar-v1-cachetest";

    fn dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("anyr-launch-cache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn model(id: &str, ctx: i64) -> CatalogModel {
        CatalogModel {
            id: id.into(),
            name: Some(format!("{id} name")),
            owned_by: Some("owner".into()),
            context_length: Some(ctx),
            max_completion_tokens: Some(8192),
            capabilities: vec!["tools".into()],
            pricing: Some(pricing()),
            created: Some(1_700_000_000),
            routable: None,
        }
    }

    fn pricing() -> crate::http::ModelPricing {
        crate::http::ModelPricing {
            input: 0.3,
            output: 1.2,
            cached_input: Some(0.03),
            list_input: Some(0.6),
            list_output: Some(2.4),
            billing: Some("credits".into()),
        }
    }

    fn ids(models: &Option<Vec<CatalogModel>>) -> Vec<String> {
        models
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|m| m.id.clone())
            .collect()
    }

    fn counting(_: &str, _: Option<&str>) -> Result<Vec<CatalogModel>, String> {
        Ok(vec![model("fetched/m", 1000)])
    }

    fn offline(_: &str, _: Option<&str>) -> Result<Vec<CatalogModel>, String> {
        panic!("this launch must not touch the network")
    }

    #[test]
    fn a_saved_catalog_reads_back_with_every_launch_field() {
        let d = dir("roundtrip");
        let cache = CatalogCache::new(&d, BASE, Some(KEY));
        cache.save(&[model("a/b", 262_144)], 1_000);
        let (models, age) = cache.load(1_000 + FRESH_SECS - 1).unwrap();
        assert_eq!(age, Age::Fresh);
        let m = &models[0];
        assert_eq!(
            (m.id.as_str(), m.name.as_deref(), m.owned_by.as_deref()),
            ("a/b", Some("a/b name"), Some("owner"))
        );
        assert_eq!(
            (m.context_length, m.max_completion_tokens),
            (Some(262_144), Some(8192))
        );
        assert_eq!(m.capabilities, vec!["tools".to_string()]);
        assert_eq!(
            (m.pricing.as_ref(), m.created),
            (Some(&pricing()), Some(1_700_000_000)),
            "the picker's price, free and new columns survive the cache"
        );
        assert_eq!(cache.load(1_000 + FRESH_SECS).unwrap().1, Age::Stale);
        assert!(
            cache.load(1_000 + STALE_SECS).is_none(),
            "a day-old copy is dropped"
        );
    }

    #[test]
    fn a_row_from_an_older_cache_version_is_ignored() {
        let d = dir("version");
        let cache = CatalogCache::new(&d, BASE, Some(KEY));
        cache.save(&[model("a/b", 1)], 1_000);
        let mut rows = read_json(&d.join(CATALOG_FILE)).unwrap();
        for row in rows.as_object_mut().unwrap().values_mut() {
            row.as_object_mut().unwrap().remove("v");
        }
        write_json(&d.join(CATALOG_FILE), &rows);
        assert!(
            cache.load(1_001).is_none(),
            "a row without prices is refetched"
        );
    }

    #[test]
    fn the_cache_never_holds_the_key_and_keys_rows_by_account() {
        let d = dir("keyed");
        CatalogCache::new(&d, BASE, Some(KEY)).save(&[model("a/b", 1)], 1_000);
        let text = std::fs::read_to_string(d.join(CATALOG_FILE)).unwrap();
        assert!(!text.contains(KEY));
        assert!(CatalogCache::new(&d, BASE, Some("sk-ar-v1-other"))
            .load(1_001)
            .is_none());
    }

    #[test]
    fn warm_launch_and_dry_run_never_fetch() {
        let d = dir("warm");
        let now = now_secs();
        CatalogCache::new(&d, BASE, Some(KEY)).save(&[model("cached/m", 1)], now);
        let warm = catalog_with(Some(&d), BASE, Some(KEY), false, now + 1, &offline);
        assert_eq!(ids(&warm.models), vec!["cached/m"]);
        assert!(warm.refresh.is_none(), "a fresh copy is not refreshed");
        let cold_dry = catalog_with(Some(&dir("cold-dry")), BASE, Some(KEY), true, now, &offline);
        assert!(cold_dry.models.is_none());
    }

    #[test]
    fn a_stale_copy_is_used_now_and_refreshed_in_the_background() {
        let d = dir("stale");
        let now = now_secs();
        CatalogCache::new(&d, BASE, Some(KEY)).save(&[model("old/m", 1)], now - FRESH_SECS - 5);
        let launch = catalog_with(Some(&d), BASE, Some(KEY), false, now, &counting);
        assert_eq!(ids(&launch.models), vec!["old/m"], "no wait on the network");
        launch.refresh.expect("refresh started").join().unwrap();
        let (models, age) = CatalogCache::new(&d, BASE, Some(KEY))
            .load(now_secs())
            .unwrap();
        assert_eq!((models[0].id.as_str(), age), ("fetched/m", Age::Fresh));
    }

    #[test]
    fn a_cold_launch_fetches_once_and_saves() {
        let d = dir("cold");
        let launch = catalog_with(Some(&d), BASE, Some(KEY), false, now_secs(), &counting);
        assert_eq!(ids(&launch.models), vec!["fetched/m"]);
        assert!(launch.refresh.is_none());
        assert!(CatalogCache::new(&d, BASE, Some(KEY))
            .load(now_secs())
            .is_some());
    }

    fn slow_full_list(_: &str, _: Option<&str>) -> Result<Vec<CatalogModel>, String> {
        std::thread::sleep(std::time::Duration::from_millis(200));
        Ok((0..30)
            .map(|n| model(&format!("full/m{n}"), 1000))
            .collect())
    }

    fn failing(_: &str, _: Option<&str>) -> Result<Vec<CatalogModel>, String> {
        Err("timed out".into())
    }

    #[test]
    fn a_list_launch_on_a_cold_cache_says_it_waits_then_saves_the_full_list() {
        let d = dir("list-cold");
        let said = std::cell::Cell::new(0);
        let launch = catalog_for_list_with(
            Some(&d),
            BASE,
            Some(KEY),
            false,
            now_secs(),
            &slow_full_list,
            &|| said.set(said.get() + 1),
        );
        assert_eq!(said.get(), 1, "one line before the wait");
        assert_eq!(launch.models.as_deref().map(<[_]>::len), Some(30));
        assert!(CatalogCache::new(&d, BASE, Some(KEY))
            .load(now_secs())
            .is_some_and(|(m, _)| m.len() == 30));
    }

    #[test]
    fn a_list_launch_with_a_copy_or_a_dry_run_does_not_announce_a_wait() {
        let say = || panic!("no wait expected");
        let d = dir("list-warm");
        let now = now_secs();
        CatalogCache::new(&d, BASE, Some(KEY)).save(&[model("cached/m", 1)], now);
        let warm = catalog_for_list_with(Some(&d), BASE, Some(KEY), false, now, &offline, &say);
        assert_eq!(ids(&warm.models), vec!["cached/m"]);
        let dry = catalog_for_list_with(
            Some(&dir("list-dry")),
            BASE,
            Some(KEY),
            true,
            now,
            &offline,
            &say,
        );
        assert!(dry.models.is_none());
    }

    #[test]
    fn a_failed_list_fetch_is_none_and_saves_nothing() {
        let d = dir("list-fail");
        let launch = catalog_for_list_with(
            Some(&d),
            BASE,
            Some(KEY),
            false,
            now_secs(),
            &failing,
            &|| {},
        );
        assert!(launch.models.is_none());
        assert!(CatalogCache::new(&d, BASE, Some(KEY))
            .load(now_secs())
            .is_none());
    }

    #[cfg(feature = "native")]
    #[test]
    fn the_list_budget_outlasts_the_launch_budget() {
        assert!(LIST_FETCH >= std::time::Duration::from_secs(8));
        assert!(LIST_FETCH > COLD_FETCH);
    }

    #[test]
    fn the_routable_flag_survives_the_cache() {
        let d = dir("routable");
        let cache = CatalogCache::new(&d, BASE, Some(KEY));
        let blocked = CatalogModel {
            routable: Some(false),
            ..model("x/blocked", 1)
        };
        cache.save(&[blocked, model("x/open", 1)], 1_000);
        let (models, _) = cache.load(1_001).unwrap();
        assert_eq!(
            models.iter().map(|m| m.routable).collect::<Vec<_>>(),
            vec![Some(false), None]
        );
    }

    #[test]
    fn concurrent_saves_always_leave_valid_json() {
        let d = dir("concurrent");
        let path = d.join(CATALOG_FILE);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut reads = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        serde_json::from_str::<Value>(&text)
                            .unwrap_or_else(|e| panic!("torn cache file ({e}): {text:?}"));
                        reads += 1;
                    }
                }
                reads
            })
        };
        let writers: Vec<_> = (0..8)
            .map(|t| {
                let d = d.clone();
                std::thread::spawn(move || {
                    // Different keys and sizes, so an in-place write would tear.
                    let cache = CatalogCache::new(&d, BASE, Some(&format!("{KEY}{t}")));
                    for i in 0..40 {
                        let models: Vec<_> = (0..(t * 7 + i % 5 + 1))
                            .map(|n| model(&format!("m/{t}-{n}"), 1000))
                            .collect();
                        cache.save(&models, now_secs());
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(reader.join().unwrap() > 0, "the reader saw the file");
        serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn opencode_version_is_probed_once_per_binary() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("oc");
        let bin = d.join("opencode");
        let log = d.join("probes");
        let script = format!("#!/bin/sh\necho x >> '{}'\necho 'v2.0.24'\n", log.display());
        std::fs::write(&bin, &script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cmd = bin.to_str().unwrap();
        assert_eq!(opencode_major(Some(&d), cmd), Some(2));
        assert_eq!(opencode_major(Some(&d), cmd), Some(2));
        let probes = || std::fs::read_to_string(&log).unwrap().lines().count();
        assert_eq!(probes(), 1, "second launch reads the cache");
        std::fs::write(&bin, script.replace("v2.0.24", "1.4.0 ")).unwrap();
        assert_eq!(
            opencode_major(Some(&d), cmd),
            Some(1),
            "an upgrade is probed again"
        );
        assert_eq!(probes(), 2);
    }
}
