//! `anyr upgrade` / `anyr upgrade --check`.
//! Network is skipped when `--fixture` or `ANYR_RELEASES_JSON` is set.

use std::collections::BTreeMap;
use std::fs;
#[cfg(feature = "native")]
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::autoupdate::{state_dir, UpdateState};
use crate::channel::{
    asset_name, channel_update_candidates, current_arch, current_os, github_token,
    merge_expanded_assets, parse_checksums, parse_releases, parse_releases_html, release_asset_url,
    releases_http_error, select_latest_release_with_asset, Channel, Release,
    GITHUB_EXPANDED_ASSETS_PREFIX, GITHUB_RELEASES_API, GITHUB_RELEASES_HTML,
};
use crate::config::{resolve_config_path, write_config};
use crate::http::{http_get_github, http_get_github_unauth, http_get_web};
use crate::key::load_config_if_present;
use crate::parse::{get_string_flag, ParsedArgs};
use crate::spawn::redact_value;
use crate::term;
use crate::VERSION;

/// True when `latest` is a higher version than `current` (leading `v` ignored).
pub fn needs_upgrade(current: &str, latest: &str) -> bool {
    match (
        crate::channel::parse_version(current),
        crate::channel::parse_version(latest),
    ) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => {
            let cur = current.trim().strip_prefix('v').unwrap_or(current.trim());
            let lat = latest.trim().strip_prefix('v').unwrap_or(latest.trim());
            !lat.is_empty() && lat != cur
        }
    }
}

/// Redact `sk-ar-` values even if the env key name does not look secret.
pub fn redact_printed_value(key: &str, value: &str) -> String {
    if value.contains("sk-ar-") {
        redact_value("API_KEY", value)
    } else {
        redact_value(key, value)
    }
}

pub fn fixture_path(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Option<String> {
    get_string_flag(&parsed.flags, "fixture")
        .or_else(|| env.get("ANYR_RELEASES_JSON").cloned())
        .filter(|s| !s.trim().is_empty())
}

/// Where `anyr update` lists GitHub Releases. Never unauthenticated REST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleasesFetch {
    Fixture,
    AuthenticatedApi,
    GithubHtml,
}

impl ReleasesFetch {
    pub fn from_context(fixture: Option<&str>, env: &BTreeMap<String, String>) -> Self {
        if fixture.is_some() {
            Self::Fixture
        } else if github_token(env).is_some() {
            Self::AuthenticatedApi
        } else {
            Self::GithubHtml
        }
    }
}

pub fn load_releases(
    fixture: Option<&str>,
    env: &BTreeMap<String, String>,
    deadline: Option<Instant>,
) -> Result<Vec<Release>, String> {
    if let Some(path) = fixture {
        let json = fs::read_to_string(path)
            .map_err(|e| format!("could not read releases fixture {path}: {e}"))?;
        return parse_releases(&json);
    }
    if let Some(base) = mock_release_base(env) {
        let (status, body) = http_get_web(&format!("{base}/releases.json"))?;
        if !(200..300).contains(&status) {
            return Err(releases_http_error(status, &body));
        }
        return parse_releases(&body);
    }
    fetch_releases(env, deadline)
}

fn fetch_releases(
    env: &BTreeMap<String, String>,
    deadline: Option<Instant>,
) -> Result<Vec<Release>, String> {
    let mut errors: Vec<String> = Vec::new();
    // 1. Authenticated REST API (when a token is configured — CI, etc.)
    if let Some(token) = github_token(env) {
        match fetch_releases_api(token) {
            Ok(rels) => return Ok(rels),
            Err(err) => errors.push(format!("GitHub REST API (with token): {err}")),
        }
    }
    // 2. Unauthenticated REST API — works for public repos from home networks
    //    (60 req/hour). More reliable than HTML scraping, which GitHub blocks
    //    for anonymous requests (returns a page without release tags).
    match fetch_releases_api_no_token() {
        Ok(rels) => return Ok(rels),
        Err(err) => errors.push(format!("GitHub REST API (unauthenticated): {err}")),
    }
    // 3. Last resort: scrape github.com/.../releases HTML.
    match fetch_releases_html(deadline) {
        Ok(rels) => Ok(rels),
        Err(html_err) => {
            let mut msg = errors.join("; ");
            if !msg.is_empty() {
                msg += "; ";
            }
            msg += &format!(
                "Fell back to github.com/releases (not the REST API), which also failed: {html_err}"
            );
            Err(msg)
        }
    }
}

/// GitHub REST API for a public repo without a token. Works from home
/// networks (unauthenticated quota: 60 req/hour). Only shared IPs (CI runners)
/// get 403 here; those keep the authenticated path above.
fn fetch_releases_api_no_token() -> Result<Vec<Release>, String> {
    let (status, body) = http_get_github_unauth(GITHUB_RELEASES_API)?;
    if !(200..300).contains(&status) {
        return Err(releases_http_error(status, &body));
    }
    parse_releases(&body)
}

fn fetch_releases_api(token: &str) -> Result<Vec<Release>, String> {
    let (status, body) = http_get_github(GITHUB_RELEASES_API, token)?;
    if !(200..300).contains(&status) {
        return Err(releases_http_error(status, &body));
    }
    parse_releases(&body)
}

fn fetch_releases_html(deadline: Option<Instant>) -> Result<Vec<Release>, String> {
    let (status, html) = http_get_web(GITHUB_RELEASES_HTML)?;
    if !(200..300).contains(&status) {
        return Err(releases_http_error(status, &html));
    }
    let mut releases = parse_releases_html(&html)?;
    for rel in &mut releases {
        if let Some(d) = deadline {
            check_deadline(d)?;
        }
        if !rel.assets.is_empty() {
            continue;
        }
        let url = format!("{GITHUB_EXPANDED_ASSETS_PREFIX}{}", rel.tag_name);
        match http_get_web(&url) {
            Ok((st, body)) if (200..300).contains(&st) => {
                merge_expanded_assets(rel, &body);
            }
            _ => {}
        }
    }
    Ok(releases)
}

/// `--beta` / `--stable` switch the persisted channel. Mutually exclusive with
/// each other and with `--channel`.
fn channel_switch_flag(parsed: &ParsedArgs) -> Result<Option<Channel>, String> {
    let beta = parsed.flag_true("beta");
    let stable = parsed.flag_true("stable");
    if beta && stable {
        return Err("Use either --beta or --stable, not both.".into());
    }
    if (beta || stable) && get_string_flag(&parsed.flags, "channel").is_some() {
        return Err("--beta/--stable cannot be combined with --channel.".into());
    }
    if beta {
        return Ok(Some(Channel::Beta));
    }
    if stable {
        return Ok(Some(Channel::Stable));
    }
    Ok(None)
}

fn resolve_channel(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<Channel, String> {
    if let Some(ch) = channel_switch_flag(parsed)? {
        return Ok(ch);
    }
    if let Some(flag) = get_string_flag(&parsed.flags, "channel") {
        return Channel::parse(&flag);
    }
    configured_channel_checked(env)
}

/// The channel this install follows, ignoring command-line flags. Config wins
/// over ANYR_CHANNEL so bare `anyr update` does not jump tracks when a stale
/// env is set. Only --beta/--stable persist a switch. An unparsable value
/// falls back to the build's own track.
pub(crate) fn configured_channel(env: &BTreeMap<String, String>) -> Channel {
    configured_channel_checked(env).unwrap_or_else(|_| channel_of_version(VERSION))
}

fn configured_channel_checked(env: &BTreeMap<String, String>) -> Result<Channel, String> {
    let path = resolve_config_path(None, env);
    if let Some(ch) = load_config_if_present(&path).and_then(|c| c.channel) {
        return Channel::parse(&ch);
    }
    if let Some(v) = env
        .get("ANYR_CHANNEL")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return Channel::parse(v);
    }
    Ok(channel_of_version(VERSION))
}

/// With no channel configured, stay on the track this build came from: a
/// beta build follows beta. Falling back to stable would strand a beta
/// install, since stable 0.1.29 sorts above 0.1.29-beta.390.
fn channel_of_version(version: &str) -> Channel {
    match crate::channel::parse_version(version) {
        Some(v) if v.prerelease.is_some() => Channel::Beta,
        _ => Channel::Stable,
    }
}

/// Persist `channel:` so future auto-updates follow the switched track.
fn persist_channel(channel: Channel, env: &BTreeMap<String, String>) -> Result<bool, String> {
    let path = resolve_config_path(None, env);
    let mut cfg = load_config_if_present(&path).unwrap_or_default();
    let next = channel.as_str().to_string();
    let changed = cfg.channel.as_deref() != Some(next.as_str());
    if !changed && cfg.channel.is_some() {
        return Ok(false);
    }
    // Also write when channel was previously unset and we are pinning stable/beta,
    // so subsequent runs don't depend on defaults alone.
    let was_unset = cfg.channel.is_none();
    cfg.channel = Some(next);
    write_config(&cfg, &path)?;
    Ok(changed || was_unset)
}

fn version_eq(a: &str, b: &str) -> bool {
    let a = a.trim().strip_prefix('v').unwrap_or(a.trim());
    let b = b.trim().strip_prefix('v').unwrap_or(b.trim());
    a == b
}

fn wants_check(parsed: &ParsedArgs) -> bool {
    parsed.flag_true("check") || parsed.passthrough.iter().any(|a| a == "check")
}

fn print_redacted_env(env: &BTreeMap<String, String>) {
    for key in [
        "ANYR_CHANNEL",
        "ANYR_RELEASES_JSON",
        "ANYROUTER_API_KEY",
        "ANYR_SETUP_BIN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "ANYR_GITHUB_TOKEN",
    ] {
        if let Some(value) = env.get(key) {
            println!(
                "{}",
                field("env", format!("{key}={}", redact_printed_value(key, value)))
            );
        }
    }
}

/// What an update run should do, given the installed version and the
/// channel's installable releases (newest first).
#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdatePlan {
    UpToDate,
    /// Installed build is newer than everything on the channel, e.g. a beta
    /// build after switching to stable. Never downgrade silently.
    Ahead {
        latest: String,
    },
    /// Install candidates, newest first.
    Install(Vec<Release>),
}

/// Only strictly newer releases are installed. A downgrade (installed build
/// ahead of the channel) needs `allow_downgrade` (`--beta/--stable --force`).
fn plan_update(current: &str, candidates: Vec<Release>, allow_downgrade: bool) -> UpdatePlan {
    let Some(latest) = candidates.first().map(|r| r.version_str().to_string()) else {
        return UpdatePlan::UpToDate;
    };
    if version_eq(current, &latest) {
        return UpdatePlan::UpToDate;
    }
    let newer: Vec<Release> = candidates
        .iter()
        .filter(|r| needs_upgrade(current, r.version_str()))
        .cloned()
        .collect();
    if !newer.is_empty() {
        return UpdatePlan::Install(newer);
    }
    if allow_downgrade {
        return UpdatePlan::Install(candidates);
    }
    UpdatePlan::Ahead { latest }
}

fn downgrade_hint(channel: Channel) -> String {
    format!(
        "{} update --{} --force",
        crate::help::invoked_bin(),
        channel.as_str()
    )
}

/// Install-time error for a failed asset download. A 404 means the selected
/// release has no binary assets uploaded yet (stable v0.1.11 shipped empty),
/// so name the release instead of surfacing a bare HTTP code.
fn download_status_error(status: u16, url: &str) -> String {
    if status == 404 {
        if let Some(tag) = url
            .split_once("/releases/download/")
            .and_then(|(_, rest)| rest.split('/').next())
            .filter(|tag| !tag.is_empty())
        {
            return format!(
                "Release {tag} has no binary assets uploaded yet. Try `{} update --beta`, \
or re-run later.",
                crate::help::invoked_bin()
            );
        }
    }
    format!("download HTTP {status} from {url}")
}

#[cfg(not(feature = "native"))]
fn download_binary(_url: &str, _dest: &Path) -> Result<(), String> {
    Err("download is not available in the browser demo".into())
}

#[cfg(feature = "native")]
fn download_binary(url: &str, dest: &Path) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(&format!("anyr-cli/{VERSION}"))
        .build();
    let resp = match agent.get(url).call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let _ = resp.into_string();
            return Err(download_status_error(code, url));
        }
        Err(err) => return Err(format!("download failed: {err}")),
    };
    if !(200..300).contains(&resp.status()) {
        return Err(download_status_error(resp.status(), url));
    }
    let mut reader = resp.into_reader();
    let mut file =
        fs::File::create(dest).map_err(|e| format!("could not write {}: {e}", dest.display()))?;
    io::copy(&mut reader, &mut file).map_err(|e| format!("could not save download: {e}"))?;
    file.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Only official release assets may replace the binary.
const RELEASE_DOWNLOAD_PREFIX: &str = "https://github.com/AnyRouterHQ/cli/releases/download/";
/// Pre-transfer owner. GitHub 301s these to the canonical prefix above, and
/// older release listings/fixtures still carry them, so accept both rather
/// than refusing a legitimate asset after the rename.
const LEGACY_RELEASE_DOWNLOAD_PREFIX: &str =
    "https://github.com/anyrouter-dev/cli/releases/download/";

/// First release that always ships `checksums.txt`. Older tags are legacy and
/// may install unverified (with a warning); newer ones fail closed.
const CHECKSUMS_REQUIRED_FROM: &str = "0.1.14";

fn validate_download_url(url: &str, env: &BTreeMap<String, String>) -> Result<(), String> {
    let mock = mock_release_base(env).filter(|base| url.starts_with(&format!("{base}/")));
    if url.starts_with(RELEASE_DOWNLOAD_PREFIX)
        || url.starts_with(LEGACY_RELEASE_DOWNLOAD_PREFIX)
        || mock.is_some()
    {
        Ok(())
    } else {
        Err(format!(
            "refusing to install from {url}: updates must come from {RELEASE_DOWNLOAD_PREFIX}"
        ))
    }
}

/// `ANYR_UPDATE_BASE_URL` points the updater at a mock release server in
/// tests. Only loopback is accepted, so it can never redirect a real install
/// to another host.
pub(crate) fn mock_release_base(env: &BTreeMap<String, String>) -> Option<String> {
    let base = env
        .get(crate::autoupdate::ENV_UPDATE_BASE_URL)?
        .trim()
        .trim_end_matches('/');
    ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|p| base.starts_with(p))
        .then(|| base.to_string())
}

fn release_tag_from_url(url: &str) -> Option<&str> {
    url.split_once("/releases/download/")
        .and_then(|(_, rest)| rest.split('/').next())
        .filter(|tag| !tag.is_empty())
}

/// Unknown or unparseable tags require checksums (fail closed).
fn checksums_required(url: &str) -> bool {
    match release_tag_from_url(url).and_then(crate::channel::parse_version) {
        Some(ver) => match crate::channel::parse_version(CHECKSUMS_REQUIRED_FROM) {
            Some(cutoff) => ver >= cutoff,
            None => true,
        },
        None => true,
    }
}

/// Version of the binary currently on disk at `path` (it may differ from the
/// running process when another install replaced it meanwhile).
#[cfg(feature = "native")]
fn on_disk_version(path: &Path) -> Option<String> {
    let out = std::process::Command::new(path)
        .arg("--version")
        .env("ANYR_NO_UPDATE", "1")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

#[cfg(not(feature = "native"))]
fn on_disk_version(_path: &Path) -> Option<String> {
    None
}

/// Refuse to overwrite `on_disk` with an older-or-equal `target` unless the
/// user explicitly asked to downgrade.
fn guard_not_older(
    on_disk: Option<&str>,
    target: &str,
    allow_downgrade: bool,
) -> Result<(), String> {
    match on_disk {
        Some(disk) if !allow_downgrade && !needs_upgrade(disk, target) => Err(format!(
            "refusing to replace anyr {disk} with {target}: installed build is not older"
        )),
        _ => Ok(()),
    }
}

/// The installed binary this process should update: `current_exe` with one
/// symlink resolved. Refuses cargo dev builds.
pub(crate) fn install_dest() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    if crate::autoupdate::is_cargo_target_build(&exe) {
        return Err(format!(
            "refusing to self-update a cargo build at {}; rebuild with cargo instead",
            exe.display()
        ));
    }
    Ok(match fs::read_link(&exe) {
        Ok(target) if target.is_absolute() => target,
        Ok(target) => exe.parent().unwrap_or(Path::new(".")).join(target),
        Err(_) => exe,
    })
}

/// Download `url` into a per-process staging file beside `dest`, verify it
/// against the release's checksums.txt and make it executable. Returns the
/// staging path and the SHA-256 it was verified as. Any failure removes the
/// partial file and leaves `dest` untouched. The caller holds the update lock.
fn download_verified(
    url: &str,
    dest: &Path,
    allow_downgrade: bool,
    env: &BTreeMap<String, String>,
) -> Result<(PathBuf, String), String> {
    validate_download_url(url, env)?;
    if let Some(target) = release_tag_from_url(url) {
        let target = target.strip_prefix('v').unwrap_or(target);
        guard_not_older(on_disk_version(dest).as_deref(), target, allow_downgrade)?;
    }
    crate::autoupdate::clear_stale_downloads(dest);
    let tmp = crate::autoupdate::sibling(dest, &format!("download.{}", std::process::id()));
    let sha = match download_binary(url, &tmp).and_then(|_| verify_downloaded_asset(url, &tmp)) {
        Ok(sha) => sha,
        Err(e) => return abort_download(&tmp, e),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)) {
            return abort_download(&tmp, format!("chmod {}: {e}", tmp.display()));
        }
    }
    Ok((tmp, sha))
}

/// `anyr update`: download, verify and swap now, keeping the old build for
/// `--rollback`.
fn replace_current_binary(
    url: &str,
    allow_downgrade: bool,
    env: &BTreeMap<String, String>,
) -> Result<PathBuf, String> {
    let dest = install_dest()?;
    let (tmp, _) = download_verified(url, &dest, allow_downgrade, env)?;
    crate::autoupdate::swap_in(&tmp, &dest).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(dest)
}

/// Background check: download and verify into `.anyr.pending`, never swap.
/// Returns the pending path and the hash it was verified as.
fn stage_binary(
    url: &str,
    dest: &Path,
    env: &BTreeMap<String, String>,
) -> Result<(PathBuf, String), String> {
    let (tmp, sha) = download_verified(url, dest, false, env)?;
    let pending = crate::autoupdate::sibling(dest, "pending");
    fs::rename(&tmp, &pending).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("could not stage {}: {e}", pending.display())
    })?;
    Ok((pending, sha))
}

/// Sibling `checksums.txt` in the same release-asset directory as `asset_url`.
fn checksums_url(asset_url: &str) -> String {
    match asset_url.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/checksums.txt"),
        None => "checksums.txt".into(),
    }
}

fn asset_name_from_url(url: &str) -> &str {
    url.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("anyr")
}

/// HTTP 404 on checksums.txt means a legacy release; skip verification.
fn checksums_missing_is_legacy(status: u16) -> bool {
    status == 404
}

fn verify_checksum_file(
    map: &BTreeMap<String, String>,
    asset_name: &str,
    actual_hex: &str,
) -> Result<(), String> {
    let Some(expected) = map.get(asset_name) else {
        return Err(format!("checksums.txt has no entry for {asset_name}"));
    };
    let expected = expected.trim().to_ascii_lowercase();
    let actual = actual_hex.trim().to_ascii_lowercase();
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch for {asset_name}: expected {expected}, got {actual}"
        ))
    }
}

fn abort_download<T>(tmp: &Path, err: String) -> Result<T, String> {
    let _ = fs::remove_file(tmp);
    Err(err)
}

pub(crate) fn sha256_hex_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("could not hash {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(not(feature = "native"))]
fn fetch_checksums_body(_url: &str) -> Result<Option<String>, String> {
    Err("download is not available in the browser demo".into())
}

#[cfg(feature = "native")]
fn fetch_checksums_body(url: &str) -> Result<Option<String>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(&format!("anyr-cli/{VERSION}"))
        .build();
    match agent.get(url).call() {
        Ok(resp) => {
            let status = resp.status();
            if checksums_missing_is_legacy(status) {
                return Ok(None);
            }
            if !(200..300).contains(&status) {
                return Err(format!("checksums download HTTP {status} from {url}"));
            }
            resp.into_string()
                .map(Some)
                .map_err(|e| format!("could not read checksums.txt: {e}"))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let _ = resp.into_string();
            if checksums_missing_is_legacy(code) {
                Ok(None)
            } else {
                Err(format!("checksums download HTTP {code} from {url}"))
            }
        }
        Err(err) => Err(format!("checksums download failed: {err}")),
    }
}

/// Errors that mean this GitHub release is unusable; try the next on the channel.
pub fn skippable_release_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("checksum")
        || e.contains("no entry for")
        || e.contains("no binary assets")
        || e.contains("download http")
        || e.contains("download failed")
        || e.contains("checksums download")
        || e.contains("could not save download")
        || e.contains("could not hash")
        || e.contains("could not write")
}

/// Newest-first install: warn and skip a broken latest (checksum / missing
/// asset / corrupt) rather than aborting the whole update.
fn try_releases<T>(
    candidates: &[Release],
    os: &str,
    arch: &str,
    mut install: impl FnMut(&str) -> Result<T, String>,
    mut warn: impl FnMut(&str),
) -> Result<(Release, T), String> {
    let mut last = String::from("no installable release on this channel");
    for (i, rel) in candidates.iter().enumerate() {
        let url = release_asset_url(rel, os, arch);
        match install(&url) {
            Ok(path) => return Ok((rel.clone(), path)),
            Err(err) if skippable_release_error(&err) => {
                last = err.clone();
                let next = candidates.get(i + 1).map(|r| r.tag_name.as_str());
                match next {
                    Some(tag) => warn(&format!(
                        "warning: skipped {} ({err}); trying {tag}",
                        rel.tag_name
                    )),
                    None => warn(&format!("warning: skipped {} ({err})", rel.tag_name)),
                }
            }
            Err(err) => return Err(err),
        }
    }
    Err(last)
}

/// Verify `tmp` against sibling `checksums.txt`. 404 → warn and skip (legacy).
/// Returns the SHA-256 of `tmp` as verified, which is what a staged build is
/// later re-checked against.
fn verify_downloaded_asset(url: &str, tmp: &Path) -> Result<String, String> {
    let checksums = checksums_url(url);
    verify_against_checksums(fetch_checksums_body(&checksums)?, url, tmp)
}

fn verify_against_checksums(body: Option<String>, url: &str, tmp: &Path) -> Result<String, String> {
    match body {
        None if checksums_required(url) => Err(format!(
            "release {} has no checksums.txt; refusing to install an unverified binary",
            release_tag_from_url(url).unwrap_or("?")
        )),
        None => {
            crate::spinner::warn_beside_spinner(
                "warning: legacy release has no checksums.txt — skipping verification",
            );
            sha256_hex_file(tmp)
        }
        Some(body) => {
            let map = parse_checksums(&body);
            let asset = asset_name_from_url(url);
            let actual = sha256_hex_file(tmp)?;
            verify_checksum_file(&map, asset, &actual)?;
            Ok(actual)
        }
    }
}

const ANOTHER_UPDATE_RUNNING: &str = "another anyr update is running; try again shortly";

/// Holds the OS lock on `update.lock` (flock on Unix, LockFileEx on
/// Windows). The kernel picks exactly one owner and frees the lock when the
/// owner exits or is killed, so there is no staleness rule and the file is
/// never deleted: unlinking it could free a newer owner.
pub(crate) struct UpdateLock {
    _file: fs::File,
}

/// Take the update lock without waiting. Err says why: another update holds
/// it, or the lock file cannot be opened or locked at all.
pub(crate) fn try_lock(env: &BTreeMap<String, String>) -> Result<UpdateLock, String> {
    let dir = state_dir(env);
    let _ = crate::config::ensure_private_dir(&dir);
    let path = dir.join("update.lock");
    let mut opts = fs::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(&path)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Err(ANOTHER_UPDATE_RUNNING.into()),
        Err(fs::TryLockError::Error(e)) => {
            return Err(format!("could not lock {}: {e}", path.display()))
        }
    }
    // The pid is for a human reading the file; nothing parses it.
    let _ = file.set_len(0);
    let _ = writeln!(file, "{}", std::process::id());
    Ok(UpdateLock { _file: file })
}

/// `anyr update --auto`, run detached by `autoupdate::on_startup`. Checks
/// the channel and stages a verified build for the next run. Never swaps the
/// binary and never prints; every failure lands in the state file.
fn run_auto(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let Ok(_lock) = try_lock(env) else {
        // Could not lock (another stager holds it, or the filesystem cannot
        // lock at all). Record the check anyway so every command does not
        // respawn a stager that exits at once until the next interval.
        let mut state = UpdateState::load(env);
        state.last_check = crate::autoupdate::now_secs();
        state.save(env);
        return Ok(0);
    };
    let mut state = UpdateState::load(env);
    if crate::autoupdate::checked_recently(&state, env) && !parsed.flag_true("force") {
        return Ok(0);
    }
    // Record the check before the network work: a stager that dies or times
    // out must not be respawned by every command until the next interval.
    state.last_check = crate::autoupdate::now_secs();
    state.save(env);
    // The old updater wrote update.stamp / update.notice; #190 stopped
    // reading them, so sweep them now that the lock proves we own the run.
    remove_legacy_stamp_files(env);
    if let Ok(dest) = install_dest() {
        crate::autoupdate::clean_trash(&dest);
    }
    state.last_error = auto_stage(parsed, env, &mut state).err();
    state.save(env);
    Ok(0)
}

/// Best-effort removal of the pre-#190 bookkeeping files.
fn remove_legacy_stamp_files(env: &BTreeMap<String, String>) {
    let dir = state_dir(env);
    let _ = fs::remove_file(dir.join("update.stamp"));
    let _ = fs::remove_file(dir.join("update.notice"));
}

/// Candidates the background stager tries, newest first. Falling further
/// back is for a person running `anyr update`.
const AUTO_MAX_CANDIDATES: usize = 2;

/// Hard stop for one background run. Each request is also capped at 60s
/// by its own timeout; the OS lock frees as soon as the owner exits.
const AUTO_DEADLINE_SECS: u64 = 5 * 60;

fn auto_deadline(env: &BTreeMap<String, String>) -> Instant {
    let secs = env
        .get("ANYR_UPDATE_DEADLINE_SECS")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(AUTO_DEADLINE_SECS);
    Instant::now() + Duration::from_secs(secs)
}

fn check_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        return Err("background update check timed out".into());
    }
    Ok(())
}

fn auto_stage(
    parsed: &ParsedArgs,
    env: &BTreeMap<String, String>,
    state: &mut UpdateState,
) -> Result<(), String> {
    let deadline = auto_deadline(env);
    let channel = resolve_channel(parsed, env)?;
    state.channel = Some(channel.as_str().into());
    let fixture = fixture_path(parsed, env);
    let releases = load_releases(fixture.as_deref(), env, Some(deadline))?;
    check_deadline(deadline)?;
    let os = current_os();
    let arch = current_arch();
    let asset = asset_name(os, arch);
    let skip = state.skip_version.clone();
    let candidates: Vec<Release> = channel_update_candidates(&releases, channel, &asset)
        .into_iter()
        .filter(|r| {
            !skip
                .as_deref()
                .is_some_and(|v| version_eq(v, r.version_str()))
        })
        .collect();
    let UpdatePlan::Install(installable) = plan_update(VERSION, candidates, false) else {
        state.latest_seen = None;
        return Ok(());
    };
    let latest = installable[0].version_str().to_string();
    state.latest_seen = Some(latest.clone());
    if fixture.is_some() || parsed.flag_true("dry-run") {
        return Ok(());
    }
    let dest = install_dest()?;
    let staged = crate::autoupdate::sibling(&dest, "pending");
    if state.pending_version.as_deref() == Some(latest.as_str()) && staged.is_file() {
        return Ok(());
    }
    if let Some(hint) = crate::autoupdate::package_manager_hint(&dest) {
        state.hint = Some(hint);
        return Ok(());
    }
    if !crate::autoupdate::dir_writable(&dest) {
        state.hint = Some(format!("sudo {} update", crate::help::invoked_bin()));
        return Err(format!("{} is not writable", dest.display()));
    }
    state.hint = None;
    let tried = &installable[..installable.len().min(AUTO_MAX_CANDIDATES)];
    let (rel, (path, sha)) = try_releases(
        tried,
        os,
        arch,
        |url| {
            check_deadline(deadline)?;
            stage_binary(url, &dest, env)
        },
        |_| {},
    )?;
    state.pending_sha256 = Some(sha);
    state.pending_path = Some(path.to_string_lossy().into_owned());
    state.pending_version = Some(rel.version_str().to_string());
    Ok(())
}

/// `anyr update --status`: what the background updater last did.
fn print_status(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    let state = UpdateState::load(env);
    let auto = crate::autoupdate::auto_update_enabled(env);
    let channel = resolve_channel(parsed, env)?;
    if parsed.flag_true("json") {
        let mut body = state.to_json();
        body["current"] = VERSION.into();
        body["auto_update"] = auto.into();
        body["channel"] = channel.as_str().into();
        println!("{body}");
        return Ok(0);
    }
    let when = match state.last_check {
        0 => "never".to_string(),
        t => format!("{}s ago", crate::autoupdate::now_secs().saturating_sub(t)),
    };
    println!("{}", field("current", VERSION));
    println!("{}", field("channel", channel.as_str()));
    println!("{}", field("auto", if auto { "on" } else { "off" }));
    println!("{}", field("checked", when));
    if let Some(v) = &state.latest_seen {
        println!("{}", field("latest", v));
    }
    if let Some(v) = &state.pending_version {
        println!(
            "{}",
            field("pending", format!("{v} (installs on next run)"))
        );
    }
    if let Some(v) = &state.skip_version {
        println!("{}", field("skipped", format!("{v} (rolled back)")));
    }
    if let Some(h) = &state.hint {
        println!("{}", field("run", h));
    }
    if let Some(e) = &state.last_error {
        println!("{}", field("error", term::warn(e)));
    }
    Ok(0)
}

/// `anyr update --rollback`: put the previous build back and skip this
/// version in auto-update until a newer one ships.
fn run_rollback(env: &BTreeMap<String, String>) -> Result<i32, String> {
    let _lock = try_lock(env)?;
    let dest = install_dest()?;
    crate::autoupdate::rollback(&dest)?;
    let mut state = UpdateState::load(env);
    if let Some(p) = state.pending_path.take() {
        crate::autoupdate::remove_own_pending(&dest, &p);
    }
    state.clear_pending();
    state.skip_version = Some(VERSION.to_string());
    state.save(env);
    let now = on_disk_version(&dest).unwrap_or_else(|| "the previous build".into());
    println!(
        "{} Rolled back {} -> {}. Auto-update skips {} until a newer release.",
        term::ok("✔"),
        display_tag(VERSION),
        display_tag(&now),
        display_tag(VERSION)
    );
    Ok(0)
}

/// Remember why the last update check failed so `anyr update --status`
/// shows it. Manual `anyr update` never prints it (like the background
/// stager, failures stay silent), but it must stay diagnosable.
fn record_update_error(env: &BTreeMap<String, String>, err: &str) {
    let mut state = UpdateState::load(env);
    state.last_error = Some(err.to_string());
    state.save(env);
}

/// The up-to-date output for the requested mode. Used when the releases
/// listing itself fails: update checks are best-effort, so a weird page
/// (rate limit, proxy, markup change) keeps silent like "no new version".
fn print_up_to_date(parsed: &ParsedArgs, channel: Channel) -> i32 {
    if wants_check(parsed) && parsed.flag_true("json") {
        let current = Release {
            tag_name: VERSION.to_string(),
            prerelease: false,
            assets: Vec::new(),
        };
        let body = serde_json::json!({
            "current": VERSION,
            "latest": VERSION,
            "channel": channel.as_str(),
            "status": "up_to_date",
            "asset": release_asset_url(&current, current_os(), current_arch()),
        });
        println!("{body}");
        return 0;
    }
    if wants_check(parsed) {
        print_version_report(channel, VERSION);
        let current = Release {
            tag_name: VERSION.to_string(),
            prerelease: false,
            assets: Vec::new(),
        };
        println!(
            "{}",
            field(
                "asset",
                release_asset_url(&current, current_os(), current_arch())
            )
        );
        println!("{}", field("status", term::ok("up to date")));
        return 0;
    }
    println!(
        "{} Already up to date ({}, {} channel)",
        term::ok("✔"),
        display_tag(VERSION),
        channel.as_str()
    );
    0
}

pub fn run(parsed: &ParsedArgs, env: &BTreeMap<String, String>) -> Result<i32, String> {
    if parsed.flag_true("auto") {
        return run_auto(parsed, env);
    }
    if parsed.flag_true("status") {
        return print_status(parsed, env);
    }
    if parsed.flag_true("rollback") {
        return run_rollback(env);
    }
    let switch = channel_switch_flag(parsed)?;
    if let Some(ch) = switch {
        // A staged build belongs to the channel it was found on: drop it so a
        // beta cannot apply after switching to stable. Only delete this
        // install's own staging file; the state file is shared.
        let _lock = try_lock(env)?;
        let changed = persist_channel(ch, env)?;
        if changed {
            println!("channel set to {}", ch.as_str());
        }
        let mut state = UpdateState::load(env);
        if let Some(p) = state.pending_path.take() {
            if let Ok(dest) = install_dest() {
                crate::autoupdate::remove_own_pending(&dest, &p);
            }
        }
        state.clear_pending();
        state.save(env);
    }
    let channel = resolve_channel(parsed, env)?;
    let fixture = fixture_path(parsed, env);
    let releases = match load_releases(fixture.as_deref(), env, None) {
        Ok(releases) => releases,
        Err(err) => {
            // Listing releases is best-effort: a weird page (rate limit,
            // markup change, proxy) must not break the command. Stay
            // silent like "no new version"; the failure is kept for
            // `anyr update --status`. Fixture errors stay loud.
            if fixture.is_some() {
                return Err(err);
            }
            record_update_error(env, &err);
            return Ok(print_up_to_date(parsed, channel));
        }
    };
    let os = current_os();
    let arch = current_arch();
    let asset = asset_name(os, arch);
    let candidates = channel_update_candidates(&releases, channel, &asset);
    let latest = candidates
        .first()
        .cloned()
        .ok_or_else(|| select_latest_release_with_asset(&releases, channel, &asset).unwrap_err())?;
    let url = release_asset_url(&latest, os, arch);
    let latest_ver = latest.version_str();
    let allow_downgrade = switch.is_some() && parsed.flag_true("force");
    let plan = plan_update(VERSION, candidates, allow_downgrade);
    let check = wants_check(parsed);
    let dry = parsed.flag_true("dry-run") || fixture.is_some();

    if check && parsed.flag_true("json") {
        let status = match &plan {
            UpdatePlan::Install(_) if needs_upgrade(VERSION, latest_ver) => "update_available",
            UpdatePlan::Install(_) => "downgrade_available",
            UpdatePlan::UpToDate => "up_to_date",
            UpdatePlan::Ahead { .. } => "ahead",
        };
        let body = serde_json::json!({
            "current": VERSION,
            "latest": latest_ver,
            "channel": channel.as_str(),
            "status": status,
            "asset": url,
        });
        println!("{body}");
        return Ok(0);
    }
    if check {
        print_version_report(channel, latest_ver);
        println!("{}", field("asset", url));
        match &plan {
            UpdatePlan::Install(_) => {
                let label = if needs_upgrade(VERSION, latest_ver) {
                    "update available"
                } else {
                    "downgrade available"
                };
                println!("{}", status_change(term::warn(label), VERSION, latest_ver));
                let mut run = format!("{} update", crate::help::invoked_bin());
                if let Some(ch) = switch {
                    run.push_str(&format!(" --{}", ch.as_str()));
                }
                if allow_downgrade {
                    run.push_str(" --force");
                }
                println!("{}", field("run", run));
            }
            UpdatePlan::UpToDate => println!("{}", field("status", term::ok("up to date"))),
            UpdatePlan::Ahead { latest } => {
                println!(
                    "{}",
                    field(
                        "status",
                        format!(
                            "{}  {VERSION} is newer than {} {latest}",
                            term::ok("ahead"),
                            channel.as_str()
                        )
                    )
                );
                println!("{}", field("note", "kept until the channel catches up"));
                println!("{}", field("run", downgrade_hint(channel)));
            }
        }
        if parsed.flag_true("dry-run") {
            print_redacted_env(env);
        }
        return Ok(0);
    }

    let installable = match plan {
        UpdatePlan::Install(list) => list,
        UpdatePlan::UpToDate => {
            println!(
                "{} Already up to date ({}, {} channel)",
                term::ok("✔"),
                display_tag(VERSION),
                channel.as_str()
            );
            return Ok(0);
        }
        UpdatePlan::Ahead { latest } => {
            println!(
                "{} Keeping {}: newer than latest {} {}. Downgrade with `{}`.",
                term::ok("✔"),
                display_tag(VERSION),
                channel.as_str(),
                display_tag(&latest),
                downgrade_hint(channel)
            );
            return Ok(0);
        }
    };

    let target_ver = installable[0].version_str().to_string();
    // The staging and swap steps share files with the background stager.
    let _lock = if dry { None } else { Some(try_lock(env)?) };
    let spinner = crate::spinner::Spinner::start(updating_line(VERSION, &target_ver, channel));
    if dry {
        spinner.succeed(&would_update_line(&target_ver));
        if parsed.flag_true("dry-run") {
            print_redacted_env(env);
        }
        return Ok(0);
    }

    match try_releases(
        &installable,
        os,
        arch,
        |url| replace_current_binary(url, allow_downgrade, env),
        crate::spinner::warn_beside_spinner,
    ) {
        Ok((rel, _)) => {
            if rel.tag_name != installable[0].tag_name {
                spinner.warn(&format!(
                    "Installed {} after a newer {} {} release failed verification.",
                    rel.tag_name,
                    channel.as_str(),
                    installable[0].tag_name
                ));
            }
            spinner.succeed(&updated_line(rel.version_str()));
            let mut state = UpdateState::load(env);
            if let Some(p) = state.pending_path.take() {
                if let Ok(dest) = install_dest() {
                    crate::autoupdate::remove_own_pending(&dest, &p);
                }
            }
            state.clear_pending();
            state.skip_version = None;
            state.save(env);
            Ok(0)
        }
        Err(err) => {
            spinner.fail();
            Err(err)
        }
    }
}

fn field(key: &str, value: impl std::fmt::Display) -> String {
    format!("{:<8} {value}", format!("{key}:"))
}

/// GitHub-tag style (`v0.1.11`). Never invents a version number.
fn display_tag(ver: &str) -> String {
    let v = ver.trim();
    if v.starts_with('v') || v.starts_with('V') {
        v.to_string()
    } else {
        format!("v{v}")
    }
}

fn updating_line(from: &str, to: &str, channel: Channel) -> String {
    format!(
        "Updating {} -> {} ({} channel)",
        display_tag(from),
        display_tag(to),
        channel.as_str()
    )
}

fn updated_line(to: &str) -> String {
    format!("Updated to {}", display_tag(to))
}

fn would_update_line(to: &str) -> String {
    format!("Would update to {}", display_tag(to))
}

fn print_version_report(channel: Channel, latest_ver: &str) {
    println!(
        "{}",
        field(
            "current",
            format!("{VERSION} (built {})", crate::buildinfo::display_time())
        )
    );
    println!("{}", field("latest", latest_ver));
    println!("{}", field("channel", channel.as_str()));
}

/// `status:  updated  0.1.0 -> 0.1.1` — `->` is only ever old version to new.
fn status_change(status: impl std::fmt::Display, from: &str, to: &str) -> String {
    field("status", format!("{status}  {from} -> {to}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autoupdate::auto_update_enabled;
    use crate::channel::{release_asset_url, select_latest, Channel, GITHUB_DOWNLOAD_PREFIX};

    const FIXTURE: &str = r#"[
  {"tag_name":"v0.1.1","prerelease":false,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/v0.1.1/anyr-linux-x86_64"}]},
  {"tag_name":"v0.1.2-beta.1","prerelease":true,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/v0.1.2-beta.1/anyr-linux-x86_64"}]}
]"#;

    fn rel(tag: &str) -> Release {
        Release {
            tag_name: tag.into(),
            prerelease: tag.contains('-'),
            assets: Vec::new(),
        }
    }

    fn tags(plan: &UpdatePlan) -> Vec<String> {
        match plan {
            UpdatePlan::Install(list) => list.iter().map(|r| r.tag_name.clone()).collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn plan_never_downgrades_beta_to_older_stable_without_force() {
        // WHY: `update --stable` on a newer beta used to report "update
        // available" for an older stable and install it.
        let plan = plan_update("0.1.16-beta.221", vec![rel("v0.1.15")], false);
        assert_eq!(
            plan,
            UpdatePlan::Ahead {
                latest: "0.1.15".into()
            }
        );
        let forced = plan_update("0.1.16-beta.221", vec![rel("v0.1.15")], true);
        assert_eq!(tags(&forced), vec!["v0.1.15"]);
    }

    #[test]
    fn plan_current_latest_beta_is_up_to_date_not_older_fallback() {
        // WHY: `update --beta` on beta.221 picked beta.218 via the fallback
        // list because it only skipped the exact current version.
        let cands = vec![
            rel("v0.1.16-beta.221"),
            rel("v0.1.16-beta.220"),
            rel("v0.1.16-beta.218"),
        ];
        assert_eq!(
            plan_update("0.1.16-beta.221", cands.clone(), false),
            UpdatePlan::UpToDate
        );
        assert_eq!(
            plan_update("0.1.16-beta.221", cands, true),
            UpdatePlan::UpToDate
        );
    }

    #[test]
    fn plan_installs_only_strictly_newer_releases() {
        let cands = vec![rel("v0.1.17"), rel("v0.1.16"), rel("v0.1.14")];
        assert_eq!(
            tags(&plan_update("0.1.15", cands, false)),
            vec!["v0.1.17", "v0.1.16"]
        );
        assert_eq!(
            plan_update("0.1.15", Vec::new(), false),
            UpdatePlan::UpToDate
        );
    }

    #[test]
    fn download_url_must_be_official_https_release() {
        // WHY: a tampered releases listing must not point the updater at an
        // arbitrary host or plain http.
        let ok = "https://github.com/AnyRouterHQ/cli/releases/download/v0.1.15/anyr-linux-x86_64";
        assert!(validate_download_url(ok, &BTreeMap::new()).is_ok());
        // Pre-transfer URLs still resolve (301) and must not be refused.
        let legacy =
            "https://github.com/anyrouter-dev/cli/releases/download/v0.1.15/anyr-linux-x86_64";
        assert!(validate_download_url(legacy, &BTreeMap::new()).is_ok());
        for bad in [
            "http://github.com/AnyRouterHQ/cli/releases/download/v0.1.15/anyr-linux-x86_64",
            "https://evil.example/AnyRouterHQ/cli/releases/download/v0.1.15/anyr",
            "https://github.com/someone-else/cli/releases/download/v0.1.15/anyr",
            "file:///tmp/anyr",
        ] {
            assert!(
                validate_download_url(bad, &BTreeMap::new()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn checksums_required_for_current_tags_only() {
        // WHY: every release since v0.1.14 ships checksums.txt, so a missing
        // file there means tampering or a broken upload, not a legacy tag.
        let url = |tag: &str| {
            format!(
                "https://github.com/anyrouter-dev/cli/releases/download/{tag}/anyr-linux-x86_64"
            )
        };
        assert!(checksums_required(&url("v0.1.14")));
        assert!(checksums_required(&url("v0.1.16-beta.221")));
        assert!(!checksums_required(&url("v0.1.12")));
        assert!(checksums_required("https://example.com/no-tag"));
    }

    #[test]
    fn mock_release_base_is_loopback_only() {
        // WHY: the test hook must not let an env var send a real install to
        // another host.
        let env = |v: &str| {
            BTreeMap::from([(
                crate::autoupdate::ENV_UPDATE_BASE_URL.to_string(),
                v.to_string(),
            )])
        };
        let local = env("http://127.0.0.1:4000/");
        assert_eq!(
            mock_release_base(&local).as_deref(),
            Some("http://127.0.0.1:4000")
        );
        assert!(validate_download_url(
            "http://127.0.0.1:4000/releases/download/v9.9.9/anyr-linux-x86_64",
            &local
        )
        .is_ok());
        assert!(validate_download_url("http://127.0.0.2:4000/x", &local).is_err());
        for bad in [
            "https://evil.example",
            "http://127.0.0.1.evil.example:80",
            "http://evil.example/?http://127.0.0.1:1",
        ] {
            assert_eq!(mock_release_base(&env(bad)), None, "{bad}");
        }
    }

    #[test]
    fn unset_channel_follows_the_running_build() {
        // WHY: a 0.1.29-beta.390 install with no `channel:` resolved to stable
        // and was then moved off beta onto stable 0.1.29.
        assert_eq!(channel_of_version("0.1.29-beta.390"), Channel::Beta);
        assert_eq!(channel_of_version("0.1.29"), Channel::Stable);
    }

    #[test]
    fn never_overwrites_a_newer_binary_on_disk() {
        // WHY: a 0.1.16-beta.221 install was replaced by 0.1.15. The running
        // process can be older than the file on disk, so compare the disk.
        assert!(guard_not_older(Some("0.1.16-beta.221"), "0.1.15", false).is_err());
        assert!(guard_not_older(Some("0.1.15"), "0.1.15", false).is_err());
        assert!(guard_not_older(Some("0.1.15"), "0.1.16-beta.224", false).is_ok());
        assert!(guard_not_older(Some("0.1.16-beta.221"), "0.1.15", true).is_ok());
        assert!(guard_not_older(None, "0.1.15", false).is_ok());
    }

    #[test]
    fn select_latest_stable_skips_beta() {
        let rel = select_latest(FIXTURE, Channel::Stable).unwrap();
        assert_eq!(rel.version_str(), "0.1.1");
        assert!(!rel.prerelease);
    }

    #[test]
    fn select_latest_beta_picks_prerelease() {
        let rel = select_latest(FIXTURE, Channel::Beta).unwrap();
        assert_eq!(rel.version_str(), "0.1.2-beta.1");
        assert!(rel.prerelease);
    }

    #[test]
    fn needs_upgrade_newer_is_true() {
        assert!(needs_upgrade("0.1.0", "0.1.1"));
    }

    #[test]
    fn needs_upgrade_same_is_false() {
        assert!(!needs_upgrade("0.1.1", "0.1.1"));
        assert!(!needs_upgrade("0.1.1", "v0.1.1"));
    }

    #[test]
    fn release_asset_url_uses_github_releases_download() {
        let rel = select_latest(FIXTURE, Channel::Stable).unwrap();
        let url = release_asset_url(&rel, "linux", "x86_64");
        assert!(
            url.contains("github.com/AnyRouterHQ/cli/releases/download/"),
            "{url}"
        );
        assert!(url.contains(GITHUB_DOWNLOAD_PREFIX), "{url}");
        assert!(url.ends_with("/anyr-linux-x86_64"), "{url}");
    }

    #[test]
    fn redact_sk_ar_keys_in_dry_run_env() {
        let redacted = redact_printed_value("ANYROUTER_API_KEY", "sk-ar-v1-secret-value");
        assert!(!redacted.contains("sk-ar-v1-secret-value"));
        assert!(redacted.contains("sk-ar-"));
    }

    // WHY: the state dir sits beside the config holding the API key, so the
    // update bookkeeping must not leave it (or its files) readable by others.
    #[cfg(unix)]
    #[test]
    fn update_state_dir_and_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let (env, dir) = isolated_home();
        // Dir exists with the default loose mode, and a stale state file is world-readable.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("update-state.json"), "{}\n").unwrap();
        fs::set_permissions(
            dir.join("update-state.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        UpdateState::default().save(&env);
        let lock = try_lock(&env).expect("lock");

        assert_eq!(mode(&dir), 0o700);
        for name in ["update-state.json", "update.lock"] {
            assert_eq!(mode(&dir.join(name)), 0o600, "{name}");
        }
        drop(lock);
        let _ = fs::remove_dir_all(&dir);
    }

    fn isolated_home() -> (BTreeMap<String, String>, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "anyr-upd-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::create_dir_all(&dir);
        let mut env = BTreeMap::new();
        env.insert("ANYROUTER_HOME".into(), dir.to_string_lossy().into_owned());
        (env, dir)
    }

    /// Make the lock file look 1 h old. The OS lock ignores mtime (the old
    /// time-based staleness rule is gone), so backdating must not matter.
    fn backdate(path: &Path) {
        let old = std::time::SystemTime::now() - Duration::from_secs(60 * 60);
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    #[test]
    fn concurrent_lockers_of_an_old_lock_file_get_one_owner() {
        // WHY: two stagers racing the OS lock on the same file must yield
        // exactly one owner, never two racing on the same pending build.
        use std::sync::{Arc, Barrier};
        let (env, dir) = isolated_home();
        let path = dir.join("update.lock");
        for _ in 0..200 {
            drop(try_lock(&env).ok());
            if !path.exists() {
                fs::write(&path, "1\n").unwrap();
            }
            backdate(&path);
            let barrier = Arc::new(Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let (env, barrier) = (env.clone(), Arc::clone(&barrier));
                    std::thread::spawn(move || {
                        barrier.wait();
                        try_lock(&env).ok()
                    })
                })
                .collect();
            let guards: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            let owners = guards.iter().filter(|g| g.is_some()).count();
            assert_eq!(owners, 1, "{owners} processes hold the update lock");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dropping_a_lock_never_frees_a_newer_owner() {
        // WHY: dropping one owner's guard must not free the file out from
        // under a newer owner and let a third one in. (No staleness rule:
        // the kernel frees the lock only when its owner exits.)
        let (env, dir) = isolated_home();
        let a = try_lock(&env).expect("first lock");
        backdate(&dir.join("update.lock"));
        let b = try_lock(&env).ok();
        drop(a);
        let c = try_lock(&env).ok();
        assert!(
            !(b.is_some() && c.is_some()),
            "two live owners after the first one dropped"
        );
        drop((b, c));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lock_left_by_a_dead_updater_is_free_at_once() {
        // WHY: the OS frees the lock when the owner dies, so a killed
        // updater never blocks the next update.
        let (env, dir) = isolated_home();
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit"][..]
            } else {
                &[][..]
            })
            .spawn()
            .unwrap();
        let dead = child.id();
        child.wait().unwrap();
        // A fresh file: the stale pid text alone must not hold the lock.
        fs::write(dir.join("update.lock"), format!("{dead}\n")).unwrap();
        assert!(try_lock(&env).is_ok(), "dead updater still holds the lock");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_auto_records_last_check_when_the_lock_is_held() {
        // WHY: a stager that cannot lock must still record the check, or
        // every command respawns one that exits at once until the interval.
        let (env, dir) = isolated_home();
        let _lock = try_lock(&env).expect("first lock");
        assert_eq!(UpdateState::load(&env).last_check, 0);
        let parsed = ParsedArgs {
            command: "update".into(),
            flags: std::collections::HashMap::from([(
                "auto".into(),
                crate::parse::FlagValue::Bool(true),
            )]),
            passthrough: Vec::new(),
            raw_from: None,
        };
        run_auto(&parsed, &env).unwrap();
        assert_ne!(
            UpdateState::load(&env).last_check,
            0,
            "lock failure must still record last_check"
        );
        drop(_lock);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_auto_removes_legacy_stamp_and_notice_files() {
        // WHY: the pre-#190 updater wrote update.stamp / update.notice, which
        // nothing reads anymore; the stager sweeps them once it holds the lock.
        let (env, dir) = isolated_home();
        fs::write(dir.join("update.stamp"), "old").unwrap();
        fs::write(dir.join("update.notice"), "old").unwrap();
        let parsed = ParsedArgs {
            command: "update".into(),
            flags: std::collections::HashMap::from([(
                "auto".into(),
                crate::parse::FlagValue::Bool(true),
            )]),
            passthrough: Vec::new(),
            raw_from: None,
        };
        run_auto(&parsed, &env).unwrap();
        assert!(!dir.join("update.stamp").exists());
        assert!(!dir.join("update.notice").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_update_enabled_defaults_on() {
        let (env, _) = isolated_home();
        assert!(auto_update_enabled(&env));
    }

    #[test]
    fn auto_update_disabled_by_env_and_ci() {
        let (mut env, _) = isolated_home();
        env.insert("ANYR_NO_UPDATE".into(), "1".into());
        assert!(!auto_update_enabled(&env));

        let (mut env, _) = isolated_home();
        env.insert("ANYR_AUTO_UPDATE".into(), "0".into());
        assert!(!auto_update_enabled(&env));

        let (mut env, _) = isolated_home();
        env.insert("CI".into(), "true".into());
        assert!(!auto_update_enabled(&env));
    }

    #[test]
    fn auto_update_env_on_overrides_nothing() {
        let (mut env, _) = isolated_home();
        env.insert("ANYR_AUTO_UPDATE".into(), "on".into());
        assert!(auto_update_enabled(&env));
    }

    #[test]
    fn auto_update_config_false_disables() {
        let (env, dir) = isolated_home();
        fs::write(
            dir.join("config.yaml"),
            "active_profile: default\nauto_update: false\nprofiles:\n  default:\n    api_key: x\n",
        )
        .unwrap();
        assert!(!auto_update_enabled(&env));
    }

    fn parsed_check() -> ParsedArgs {
        ParsedArgs {
            command: "upgrade".into(),
            flags: std::collections::HashMap::from([(
                "check".into(),
                crate::parse::FlagValue::Bool(true),
            )]),
            passthrough: Vec::new(),
            raw_from: None,
        }
    }

    #[test]
    fn resolve_channel_reads_config_then_env() {
        let (mut env, dir) = isolated_home();
        fs::write(
            dir.join("config.yaml"),
            "active_profile: default\nchannel: beta\nprofiles:\n  default:\n    api_key: x\n",
        )
        .unwrap();
        assert_eq!(
            resolve_channel(&parsed_check(), &env).unwrap(),
            Channel::Beta
        );
        env.insert("ANYR_CHANNEL".into(), "stable".into());
        assert_eq!(
            resolve_channel(&parsed_check(), &env).unwrap(),
            Channel::Beta,
            "bare update must keep config channel over ANYR_CHANNEL"
        );
        let (mut env, _) = isolated_home();
        env.insert("ANYR_CHANNEL".into(), "beta".into());
        assert_eq!(
            resolve_channel(&parsed_check(), &env).unwrap(),
            Channel::Beta
        );
    }

    fn parsed_with(flags: &[(&str, bool)]) -> ParsedArgs {
        ParsedArgs {
            command: "update".into(),
            flags: flags
                .iter()
                .map(|(k, v)| ((*k).into(), crate::parse::FlagValue::Bool(*v)))
                .collect(),
            passthrough: Vec::new(),
            raw_from: None,
        }
    }

    #[test]
    fn beta_stable_flags_resolve_and_conflict() {
        assert_eq!(
            resolve_channel(&parsed_with(&[("beta", true)]), &BTreeMap::new()).unwrap(),
            Channel::Beta
        );
        assert_eq!(
            resolve_channel(&parsed_with(&[("stable", true)]), &BTreeMap::new()).unwrap(),
            Channel::Stable
        );
        let err =
            channel_switch_flag(&parsed_with(&[("beta", true), ("stable", true)])).unwrap_err();
        assert!(err.contains("either --beta or --stable"), "{err}");
    }

    #[test]
    fn beta_flag_conflicts_with_channel() {
        let parsed = ParsedArgs {
            command: "update".into(),
            flags: std::collections::HashMap::from([
                ("beta".into(), crate::parse::FlagValue::Bool(true)),
                (
                    "channel".into(),
                    crate::parse::FlagValue::Value("stable".into()),
                ),
            ]),
            passthrough: Vec::new(),
            raw_from: None,
        };
        let err = channel_switch_flag(&parsed).unwrap_err();
        assert!(err.contains("--channel"), "{err}");
    }

    #[test]
    fn persist_channel_writes_config() {
        let (env, dir) = isolated_home();
        assert!(persist_channel(Channel::Beta, &env).unwrap());
        let raw = fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(raw.contains("channel: beta"), "{raw}");
        assert!(!persist_channel(Channel::Beta, &env).unwrap());
        assert!(persist_channel(Channel::Stable, &env).unwrap());
        let raw = fs::read_to_string(dir.join("config.yaml")).unwrap();
        assert!(raw.contains("channel: stable"), "{raw}");
    }

    #[test]
    fn version_eq_ignores_v_prefix() {
        assert!(version_eq("0.1.1", "v0.1.1"));
        assert!(!version_eq("0.1.1", "0.1.2"));
    }

    #[test]
    fn field_aligns_values_and_keeps_colon() {
        assert_eq!(field("current", "0.1.0"), "current: 0.1.0");
        assert_eq!(field("latest", "0.1.1"), "latest:  0.1.1");
        assert_eq!(field("channel", "beta"), "channel: beta");
        assert_eq!(field("status", "up to date"), "status:  up to date");
        assert_eq!(
            field("path", "/home/duyet/.local/bin/anyr"),
            "path:    /home/duyet/.local/bin/anyr"
        );
    }

    #[test]
    fn status_change_is_old_then_new_not_a_path() {
        let line = status_change("updated", "0.1.11-beta.70", "0.1.11-beta.76");
        assert_eq!(line, "status:  updated  0.1.11-beta.70 -> 0.1.11-beta.76");
        assert!(
            !line.contains("/"),
            "status must not reuse -> for an install path: {line}"
        );
    }

    #[test]
    fn updating_line_shows_from_to_and_channel() {
        let line = updating_line("0.1.11", "0.1.99", Channel::Stable);
        assert_eq!(line, "Updating v0.1.11 -> v0.1.99 (stable channel)");
        let beta = updating_line("v0.1.11", "0.2.0-beta.1", Channel::Beta);
        assert_eq!(beta, "Updating v0.1.11 -> v0.2.0-beta.1 (beta channel)");
        assert_eq!(updated_line("0.1.99"), "Updated to v0.1.99");
        assert_eq!(
            would_update_line("0.2.0-beta.1"),
            "Would update to v0.2.0-beta.1"
        );
        assert_eq!(display_tag("v0.1.11"), "v0.1.11");
    }

    #[test]
    fn download_404_names_release_and_suggests_beta() {
        let url =
            "https://github.com/anyrouter-dev/cli/releases/download/v0.1.11/anyr-linux-x86_64";
        let err = download_status_error(404, url);
        assert!(err.contains("v0.1.11"), "{err}");
        assert!(err.contains("no binary assets uploaded yet"), "{err}");
        assert!(err.contains("update --beta"), "{err}");
        // Never leak the bare HTTP line for the known case.
        assert!(!err.contains("HTTP 404"), "{err}");
    }

    #[test]
    fn download_other_statuses_keep_plain_http_error() {
        let url =
            "https://github.com/anyrouter-dev/cli/releases/download/v0.1.11/anyr-linux-x86_64";
        assert_eq!(
            download_status_error(500, url),
            format!("download HTTP 500 from {url}")
        );
    }

    #[test]
    fn releases_fetch_never_uses_unauthenticated_api() {
        let fixture = ReleasesFetch::from_context(Some("x.json"), &BTreeMap::new());
        match fixture {
            ReleasesFetch::Fixture => {}
            other => panic!("expected Fixture, got {other:?}"),
        }

        let mut env = BTreeMap::new();
        env.insert("GH_TOKEN".into(), "ghs_test".into());
        match ReleasesFetch::from_context(None, &env) {
            ReleasesFetch::AuthenticatedApi => {}
            other => panic!("expected AuthenticatedApi, got {other:?}"),
        }

        match ReleasesFetch::from_context(None, &BTreeMap::new()) {
            ReleasesFetch::GithubHtml => {}
            other => panic!("token-less fetch must not hit unauth REST, got {other:?}"),
        }

        fn label(src: ReleasesFetch) -> &'static str {
            match src {
                ReleasesFetch::Fixture => "fixture",
                ReleasesFetch::AuthenticatedApi => "api",
                ReleasesFetch::GithubHtml => "html",
            }
        }
        assert_eq!(label(ReleasesFetch::GithubHtml), "html");
    }

    #[test]
    fn load_releases_fixture_skips_empty_stable_for_linux_x86_64() {
        let json = r#"[
  {"tag_name":"v0.1.11","prerelease":false,"assets":[]},
  {"tag_name":"v0.1.12-beta.98","prerelease":true,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/anyrouter-dev/cli/releases/download/v0.1.12-beta.98/anyr-linux-x86_64"}]}
]"#;
        let dir = std::env::temp_dir().join(format!(
            "anyr-empty-stable-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("releases.json");
        fs::write(&path, json).unwrap();
        let rels = load_releases(Some(path.to_str().unwrap()), &BTreeMap::new(), None).unwrap();
        let err = select_latest_release_with_asset(&rels, Channel::Stable, "anyr-linux-x86_64")
            .unwrap_err();
        assert_ne!(err, "GitHub Releases API HTTP 403");
        assert!(err.contains("update --beta"), "{err}");
        let beta =
            select_latest_release_with_asset(&rels, Channel::Beta, "anyr-linux-x86_64").unwrap();
        assert_eq!(beta.tag_name, "v0.1.12-beta.98");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksums_url_is_sibling_of_asset() {
        assert_eq!(
            checksums_url(
                "https://github.com/anyrouter-dev/cli/releases/download/v0.1.11/anyr-linux-x86_64"
            ),
            "https://github.com/anyrouter-dev/cli/releases/download/v0.1.11/checksums.txt"
        );
    }

    #[test]
    fn checksums_missing_is_legacy_on_404() {
        assert!(checksums_missing_is_legacy(404));
        assert!(!checksums_missing_is_legacy(200));
        assert!(!checksums_missing_is_legacy(500));
    }

    #[test]
    fn verify_checksum_file_match() {
        let mut map = BTreeMap::new();
        map.insert("anyr-linux-x86_64".into(), "AbCdef".into());
        assert!(verify_checksum_file(&map, "anyr-linux-x86_64", "abcdef").is_ok());
    }

    #[test]
    fn verify_checksum_file_mismatch_mentions_both_hashes() {
        let mut map = BTreeMap::new();
        map.insert("anyr-linux-x86_64".into(), "expectedhash".into());
        let err = verify_checksum_file(&map, "anyr-linux-x86_64", "actualhash").unwrap_err();
        assert!(err.contains("expectedhash"), "{err}");
        assert!(err.contains("actualhash"), "{err}");
        assert!(err.contains("anyr-linux-x86_64"), "{err}");
        assert!(
            !err.contains('\0') && !err.contains("payload"),
            "mismatch must not dump downloaded bytes: {err}"
        );
    }

    #[test]
    fn verify_checksum_file_missing_entry_names_asset() {
        let map = BTreeMap::new();
        let err = verify_checksum_file(&map, "anyr-linux-x86_64", "abc").unwrap_err();
        assert!(err.contains("anyr-linux-x86_64"), "{err}");
    }

    #[test]
    fn abort_download_removes_temp_and_keeps_hashes_only() {
        let (_env, dir) = isolated_home();
        let tmp = dir.join(".anyr.new");
        fs::write(&tmp, b"downloaded-bytes-must-not-appear-in-error").unwrap();
        let err = abort_download::<()>(
            &tmp,
            "checksum mismatch for anyr-linux-x86_64: expected aaa, got bbb".into(),
        )
        .unwrap_err();
        assert!(!tmp.exists(), "temp file must be removed on checksum abort");
        assert!(err.contains("aaa") && err.contains("bbb"), "{err}");
        assert!(
            !err.contains("downloaded-bytes-must-not-appear-in-error"),
            "mismatch must not dump downloaded bytes: {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skippable_release_error_covers_checksum_and_missing() {
        assert!(skippable_release_error(
            "checksum mismatch for anyr-linux-x86_64: expected 097463, got 330c0ada"
        ));
        assert!(skippable_release_error(
            "checksums.txt has no entry for anyr-linux-x86_64"
        ));
        assert!(skippable_release_error(
            "Release v0.1.11 has no binary assets uploaded yet. Try `anyr update --beta`."
        ));
        assert!(skippable_release_error(
            "download HTTP 500 from https://example"
        ));
        assert!(!skippable_release_error(
            "could not replace /home/me/.local/bin/anyr: permission denied"
        ));
    }

    #[test]
    fn try_releases_skips_checksum_mismatch_and_installs_next() {
        let rels = vec![
            Release {
                tag_name: "v0.1.14".into(),
                prerelease: false,
                assets: vec![crate::channel::ReleaseAsset {
                    name: "anyr-linux-x86_64".into(),
                    browser_download_url: "https://example/v0.1.14/anyr-linux-x86_64".into(),
                }],
            },
            Release {
                tag_name: "v0.1.13".into(),
                prerelease: false,
                assets: vec![crate::channel::ReleaseAsset {
                    name: "anyr-linux-x86_64".into(),
                    browser_download_url: "https://example/v0.1.13/anyr-linux-x86_64".into(),
                }],
            },
        ];
        let mut warns = Vec::new();
        let (picked, path) = try_releases(
            &rels,
            "linux",
            "x86_64",
            |url| {
                if url.contains("0.1.14") {
                    Err(
                        "checksum mismatch for anyr-linux-x86_64: expected 097463, got 330c0ada"
                            .into(),
                    )
                } else {
                    Ok(PathBuf::from("/tmp/anyr"))
                }
            },
            |w| warns.push(w.to_string()),
        )
        .unwrap();
        assert_eq!(picked.tag_name, "v0.1.13");
        assert_eq!(path, PathBuf::from("/tmp/anyr"));
        assert!(
            warns
                .iter()
                .any(|w| w.contains("v0.1.14") && w.contains("trying v0.1.13")),
            "{warns:?}"
        );
    }

    #[test]
    fn verified_hash_is_the_one_the_checksum_matched() {
        // WHY: a staged build is re-checked against this hash before the swap;
        // hashing it again after the rename would bless a file swapped in
        // between verification and staging.
        let dir = std::env::temp_dir().join(format!("anyr-verify-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("asset");
        fs::write(&file, b"build").unwrap();
        let want = sha256_hex_file(&file).unwrap();
        let url = "https://github.com/anyrouter-dev/cli/releases/download/v0.1.31/anyr-x";
        let body = format!("{want}  anyr-x\n");
        assert_eq!(
            verify_against_checksums(Some(body), url, &file).unwrap(),
            want
        );
        let wrong = format!("{}  anyr-x\n", "0".repeat(64));
        assert!(verify_against_checksums(Some(wrong), url, &file).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
