//! GitHub Release channels (stable / beta) and asset URL helpers.
//! Pure: parse fixture JSON, pick a release, build download URLs. No network.

use std::cmp::Ordering;
use std::collections::BTreeMap;

pub const GITHUB_REPO: &str = "AnyRouterHQ/cli";
pub const GITHUB_RELEASES_API: &str = "https://api.github.com/repos/AnyRouterHQ/cli/releases";
pub const GITHUB_RELEASES_HTML: &str = "https://github.com/AnyRouterHQ/cli/releases";
pub const GITHUB_EXPANDED_ASSETS_PREFIX: &str =
    "https://github.com/AnyRouterHQ/cli/releases/expanded_assets/";
pub const GITHUB_DOWNLOAD_PREFIX: &str = "https://github.com/AnyRouterHQ/cli/releases/download/";
pub const GITHUB_LATEST_DOWNLOAD: &str =
    "https://github.com/AnyRouterHQ/cli/releases/latest/download";

/// Token for `api.github.com`. Never send these to AnyRouter.
pub fn github_token(env: &BTreeMap<String, String>) -> Option<&str> {
    for key in ["GH_TOKEN", "GITHUB_TOKEN", "ANYR_GITHUB_TOKEN"] {
        if let Some(v) = env.get(key).map(|s| s.trim()).filter(|s| !s.is_empty()) {
            return Some(v);
        }
    }
    None
}

/// Actionable GitHub Releases HTTP error. Never the bare
/// `GitHub Releases API HTTP 403` line (rate-limited unauth quota).
pub fn releases_http_error(status: u16, body: &str) -> String {
    let snippet: String = body
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect();
    let rate_limited = status == 403 || status == 429;
    let hint = if rate_limited {
        " Unauthenticated api.github.com requests are rate-limited from some networks. \
Set GH_TOKEN or GITHUB_TOKEN (public-repo scope) and retry, or install with: \
curl -fsSL https://anyrouter.dev/cli.sh | bash"
    } else {
        ""
    };
    if snippet.is_empty() {
        format!("GitHub Releases returned HTTP {status}.{hint}")
    } else {
        format!("GitHub Releases returned HTTP {status}: {snippet}.{hint}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "stable" | "latest" => Ok(Channel::Stable),
            "beta" | "pre" | "prerelease" => Ok(Channel::Beta),
            other => Err(format!("Unknown channel \"{other}\". Use stable or beta.")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }

    pub fn from_env(env: &BTreeMap<String, String>) -> Result<Self, String> {
        match env.get("ANYR_CHANNEL") {
            Some(v) if !v.trim().is_empty() => Channel::parse(v),
            _ => Ok(Channel::Stable),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag_name: String,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

impl Release {
    pub fn version_str(&self) -> &str {
        self.tag_name.strip_prefix('v').unwrap_or(&self.tag_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        parse_version(s)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)) {
            Ordering::Equal => match (&self.prerelease, &other.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => cmp_prerelease(a, b),
            },
            other => other,
        }
    }
}

fn cmp_prerelease(a: &str, b: &str) -> Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();
    let n = a_parts.len().max(b_parts.len());
    for i in 0..n {
        match (a_parts.get(i), b_parts.get(i)) {
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(xn), Ok(yn)) => xn.cmp(&yn),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => (*x).cmp(*y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (None, None) => break,
        }
    }
    Ordering::Equal
}

pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim().strip_prefix('v').unwrap_or(s.trim());
    if s.is_empty() {
        return None;
    }
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p.to_string())),
        None => (s, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some(Version {
        major,
        minor,
        patch,
        prerelease: pre.filter(|p| !p.is_empty()),
    })
}

pub fn current_os() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

pub fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    }
}

/// Release asset for this build. A static musl build updates to the musl
/// asset, never to the glibc one it may not be able to run.
pub fn asset_name(os: &str, arch: &str) -> String {
    asset_name_for(os, arch, cfg!(target_env = "musl"))
}

/// `anyr-{os}-{arch}`, plus `-musl` for the static Linux build.
pub fn asset_name_for(os: &str, arch: &str, musl: bool) -> String {
    if musl && os == "linux" {
        format!("anyr-{os}-{arch}-musl")
    } else {
        format!("anyr-{os}-{arch}")
    }
}

fn is_musl_asset(asset: &str) -> bool {
    asset.ends_with("-musl")
}

pub fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid releases JSON: {e}"))?;
    let items: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(arr) => arr,
        serde_json::Value::Object(_) => vec![value],
        _ => return Err("releases JSON must be an array or object".into()),
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.get("draft").and_then(|d| d.as_bool()) == Some(true) {
            continue;
        }
        let Some(tag) = item.get("tag_name").and_then(|t| t.as_str()) else {
            continue;
        };
        let prerelease = item
            .get("prerelease")
            .and_then(|p| p.as_bool())
            .unwrap_or(false);
        let assets = item
            .get("assets")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| {
                        let name = a.get("name")?.as_str()?.to_string();
                        let url = a.get("browser_download_url")?.as_str()?.to_string();
                        Some(ReleaseAsset {
                            name,
                            browser_download_url: url,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(Release {
            tag_name: tag.to_string(),
            prerelease,
            assets,
        });
    }
    Ok(out)
}

/// Latest release for `channel` from a GitHub `/releases` JSON array.
/// Stable ignores prereleases; beta picks the highest tag of any kind, so a
/// stable cut after the last beta (0.1.21 > 0.1.21-beta.255) still reaches it.
pub fn select_latest(json: &str, channel: Channel) -> Result<Release, String> {
    let releases = parse_releases(json)?;
    select_latest_release(&releases, channel)
}

pub fn select_latest_release(releases: &[Release], channel: Channel) -> Result<Release, String> {
    let mut best: Option<(&Release, Version)> = None;
    for rel in releases {
        let matches = match channel {
            Channel::Stable => !rel.prerelease,
            Channel::Beta => true,
        };
        if !matches {
            continue;
        }
        let Some(ver) = parse_version(&rel.tag_name) else {
            continue;
        };
        let take = match &best {
            None => true,
            Some((_, cur)) => ver > *cur,
        };
        if take {
            best = Some((rel, ver));
        }
    }
    best.map(|(r, _)| r.clone()).ok_or_else(|| match channel {
        Channel::Stable => "no stable release found".into(),
        Channel::Beta => "no release found".into(),
    })
}

fn channel_matches(rel: &Release, channel: Channel) -> bool {
    match channel {
        Channel::Stable => !rel.prerelease,
        Channel::Beta => true,
    }
}

/// Newest-first releases on `channel` that look installable.
/// Prefers rows that list `asset`; otherwise any non-empty asset list.
/// Empty-asset stables such as v0.1.11 are omitted so we do not 404 `/latest`.
pub fn channel_update_candidates(
    releases: &[Release],
    channel: Channel,
    asset: &str,
) -> Vec<Release> {
    let mut named: Vec<(Version, Release)> = releases
        .iter()
        .filter(|rel| channel_matches(rel, channel))
        .filter(|rel| rel.assets.iter().any(|a| a.name == asset))
        .filter_map(|rel| parse_version(&rel.tag_name).map(|v| (v, rel.clone())))
        .collect();
    // The any-assets fallback covers listings without asset names. A musl
    // build skips it: a release without the musl asset only has glibc ones.
    if named.is_empty() && !is_musl_asset(asset) {
        named = releases
            .iter()
            .filter(|rel| channel_matches(rel, channel))
            .filter(|rel| !rel.assets.is_empty())
            .filter_map(|rel| parse_version(&rel.tag_name).map(|v| (v, rel.clone())))
            .collect();
    }
    crate::sort::sort_by(&mut named, |a, b| b.0.cmp(&a.0));
    named.into_iter().map(|(_, r)| r).collect()
}

/// Latest release on `channel` that has binaries.
/// Prefers a release that lists `asset`; otherwise any non-empty asset list.
/// Empty-asset stables such as v0.1.11 are skipped so we do not 404 `/latest`.
pub fn select_latest_release_with_asset(
    releases: &[Release],
    channel: Channel,
    asset: &str,
) -> Result<Release, String> {
    channel_update_candidates(releases, channel, asset)
        .into_iter()
        .next()
        .ok_or_else(|| match channel {
            _ if is_musl_asset(asset) => format!(
                "No {} release has {asset} yet; older releases ship glibc builds only. \
Keep this build, or install the glibc one with ANYR_LIBC=gnu and \
curl -fsSL https://anyrouter.dev/cli.sh | bash",
                channel.as_str()
            ),
            Channel::Stable => format!(
                "No stable GitHub release has {asset} (latest non-prerelease may be empty). \
Try `anyr update --beta`."
            ),
            Channel::Beta => format!("No release has {asset}."),
        })
}

fn href_end(s: &str) -> usize {
    s.find(|c: char| {
        matches!(
            c,
            '"' | '\'' | '<' | '>' | ' ' | '\n' | '\r' | '\t' | '#' | '?'
        )
    })
    .unwrap_or(s.len())
}

fn find_all_after<'a>(html: &'a str, prefix: &str) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = html[search_from..].find(prefix) {
        let start = search_from + rel + prefix.len();
        let after = &html[start..];
        let end = href_end(after);
        if end > 0 {
            out.push((start, &after[..end]));
        }
        search_from = start + end.max(1);
    }
    out
}

/// Tag names and prerelease flags from a GitHub `/releases` HTML page.
/// Owner-agnostic (`releases/tag/` only): the repo transferred from
/// anyrouter-dev/cli to AnyRouterHQ/cli, and the old parser matched the
/// old owner prefix, so the renamed page parsed to zero tags.
pub fn parse_release_tags_html(html: &str) -> Vec<(String, bool)> {
    let found = find_all_after(html, "releases/tag/");
    let mut out = Vec::new();
    let mut seen = BTreeMap::new();
    for (i, (start, tag)) in found.iter().enumerate() {
        if !tag.starts_with('v') || tag.contains('/') {
            continue;
        }
        if seen.contains_key(*tag) {
            continue;
        }
        let after_tag = start + tag.len();
        let next_start = found
            .iter()
            .skip(i + 1)
            .map(|(s, _)| *s)
            .find(|s| *s > after_tag)
            .unwrap_or(html.len());
        let window = &html[after_tag..next_start.min(html.len())];
        // Hyphen tags are this repo's prereleases; also honor GitHub's label
        // in the block that follows this tag (not the previous release).
        let prerelease = tag.contains('-') || window.contains("Pre-release");
        seen.insert(tag.to_string(), prerelease);
        out.push((tag.to_string(), prerelease));
    }
    out
}

/// Asset hrefs from a release page or `expanded_assets` HTML fragment.
/// Owner-agnostic for the same repo-transfer reason as the tag parser.
pub fn parse_download_hrefs(html: &str) -> BTreeMap<String, Vec<ReleaseAsset>> {
    let mut by_tag: BTreeMap<String, Vec<ReleaseAsset>> = BTreeMap::new();
    for (_, spec) in find_all_after(html, "releases/download/") {
        let Some((tag, name)) = spec.split_once('/') else {
            continue;
        };
        if tag.is_empty() || name.is_empty() {
            continue;
        }
        let url = format!("{GITHUB_DOWNLOAD_PREFIX}{tag}/{name}");
        let assets = by_tag.entry(tag.to_string()).or_default();
        if assets.iter().any(|a| a.name == name) {
            continue;
        }
        assets.push(ReleaseAsset {
            name: name.to_string(),
            browser_download_url: url,
        });
    }
    by_tag
}

/// GitHub `/releases` HTML → `Release` list (tags + any download hrefs present).
pub fn parse_releases_html(html: &str) -> Result<Vec<Release>, String> {
    let mut by_tag = parse_download_hrefs(html);
    let tags = parse_release_tags_html(html);
    if tags.is_empty() && by_tag.is_empty() {
        return Err("no GitHub release tags found in HTML".into());
    }
    let mut out = Vec::new();
    for (tag, prerelease) in tags {
        let assets = by_tag.remove(&tag).unwrap_or_default();
        out.push(Release {
            tag_name: tag,
            prerelease,
            assets,
        });
    }
    for (tag, assets) in by_tag {
        let prerelease = tag.contains('-');
        out.push(Release {
            tag_name: tag,
            prerelease,
            assets,
        });
    }
    Ok(out)
}

pub fn merge_expanded_assets(release: &mut Release, html: &str) {
    let Some(assets) = parse_download_hrefs(html).remove(&release.tag_name) else {
        return;
    };
    if !assets.is_empty() {
        release.assets = assets;
    }
}

/// Download URL for `anyr-{os}-{arch}` on this release.
/// Prefers `browser_download_url` from the asset list; otherwise constructs
/// `https://github.com/AnyRouterHQ/cli/releases/download/{tag}/{asset}`.
pub fn release_asset_url(release: &Release, os: &str, arch: &str) -> String {
    let name = asset_name(os, arch);
    if let Some(asset) = release.assets.iter().find(|a| a.name == name) {
        if !asset.browser_download_url.is_empty() {
            return asset.browser_download_url.clone();
        }
    }
    let tag = if release.tag_name.starts_with('v') {
        release.tag_name.clone()
    } else {
        format!("v{}", release.tag_name)
    };
    format!("{GITHUB_DOWNLOAD_PREFIX}{tag}/{name}")
}

pub fn latest_stable_download_url(os: &str, arch: &str) -> String {
    format!("{GITHUB_LATEST_DOWNLOAD}/{}", asset_name(os, arch))
}

/// One line per asset: `"<64 lowercase hex>  <name>"` (sha256sum format).
pub fn parse_checksums(body: &str) -> BTreeMap<String, String> {
    crate::sort::map(body.lines().filter_map(|line| {
        let (hex, name) = line.split_once("  ")?;
        let hex = hex.trim();
        let name = name.trim();
        (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| (name.to_string(), hex.to_ascii_lowercase()))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_parse_stable_and_beta() {
        assert_eq!(Channel::parse("stable").unwrap(), Channel::Stable);
        assert_eq!(Channel::parse("BETA").unwrap(), Channel::Beta);
        assert!(Channel::parse("nightly").is_err());
    }

    #[test]
    fn version_prerelease_is_less_than_release() {
        let rel = parse_version("0.1.1").unwrap();
        let pre = parse_version("v0.1.1-beta.1").unwrap();
        assert!(pre < rel);
        assert!(parse_version("0.1.2-beta.1").unwrap() > rel);
    }

    const FIXTURE: &str = include_str!("../tests/fixtures/releases.json");

    #[test]
    fn select_latest_stable_skips_prerelease() {
        let rel = select_latest(FIXTURE, Channel::Stable).unwrap();
        assert_eq!(rel.version_str(), "0.1.99");
        assert!(!rel.prerelease);
    }

    #[test]
    fn select_latest_beta_picks_prerelease() {
        let rel = select_latest(FIXTURE, Channel::Beta).unwrap();
        assert_eq!(rel.version_str(), "0.2.0-beta.1");
        assert!(rel.prerelease);
    }

    #[test]
    fn latest_stable_download_url_is_public_github() {
        let url = latest_stable_download_url("linux", "x86_64");
        assert_eq!(
            url,
            "https://github.com/AnyRouterHQ/cli/releases/latest/download/anyr-linux-x86_64"
        );
        assert!(!url.contains("duyet/anyrouter"));
    }

    #[test]
    fn parse_checksums_valid_line() {
        let map = parse_checksums(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  anyr-linux-x86_64\n",
        );
        assert_eq!(
            map.get("anyr-linux-x86_64").map(String::as_str),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn parse_checksums_skips_garbage() {
        let map = parse_checksums(
            "# comment\nnot a checksum\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  anyr-linux-x86_64\nshort  name\n",
        );
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("anyr-linux-x86_64"));
    }

    #[test]
    fn parse_checksums_normalizes_uppercase_hex() {
        let map = parse_checksums(
            "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF  anyr-linux-x86_64\n",
        );
        assert_eq!(
            map.get("anyr-linux-x86_64").map(String::as_str),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn from_env_defaults_stable() {
        let env = BTreeMap::new();
        assert_eq!(Channel::from_env(&env).unwrap(), Channel::Stable);
        let mut env = BTreeMap::new();
        env.insert("ANYR_CHANNEL".into(), "beta".into());
        assert_eq!(Channel::from_env(&env).unwrap(), Channel::Beta);
    }

    #[test]
    fn github_token_prefers_gh_token() {
        let mut env = BTreeMap::new();
        env.insert("GITHUB_TOKEN".into(), "ghs_other".into());
        env.insert("GH_TOKEN".into(), "ghs_preferred".into());
        assert_eq!(github_token(&env), Some("ghs_preferred"));
        let empty = BTreeMap::new();
        assert_eq!(github_token(&empty), None);
    }

    #[test]
    fn releases_http_error_403_is_not_bare_api_line() {
        let err = releases_http_error(403, "API rate limit exceeded for 1.2.3.4");
        assert_ne!(err, "GitHub Releases API HTTP 403");
        assert!(!err.starts_with("GitHub Releases API HTTP "), "{err}");
        assert!(err.contains("403"), "{err}");
        assert!(
            err.contains("GH_TOKEN") || err.contains("GITHUB_TOKEN"),
            "{err}"
        );
        assert!(err.contains("rate-limited"), "{err}");
        assert!(err.contains("cli.sh"), "{err}");
    }

    #[test]
    fn releases_http_error_other_status_keeps_code() {
        let err = releases_http_error(500, "");
        assert!(err.contains("500"), "{err}");
        assert_ne!(err, "GitHub Releases API HTTP 500");
        assert!(!err.contains("GH_TOKEN"), "{err}");
    }

    const HTML_LISTING: &str = r#"
<a href="/anyrouter-dev/cli/releases/tag/v0.1.11">v0.1.11</a>
<span>Latest</span>
<a href="/anyrouter-dev/cli/releases/tag/v0.1.12-beta.98">v0.1.12-beta.98</a>
<span class="Label">Pre-release</span>
<a href="/anyrouter-dev/cli/releases/download/v0.1.12-beta.98/anyr-linux-x86_64">anyr-linux-x86_64</a>
<a href="https://github.com/anyrouter-dev/cli/releases/download/v0.1.12-beta.98/anyr-darwin-arm64">anyr-darwin-arm64</a>
"#;

    #[test]
    fn parse_releases_html_skips_empty_stable_and_keeps_beta_assets() {
        let rels = parse_releases_html(HTML_LISTING).unwrap();
        let stable = rels.iter().find(|r| r.tag_name == "v0.1.11").unwrap();
        assert!(!stable.prerelease);
        assert!(stable.assets.is_empty(), "{stable:?}");
        let beta = rels
            .iter()
            .find(|r| r.tag_name == "v0.1.12-beta.98")
            .unwrap();
        assert!(beta.prerelease);
        assert!(
            beta.assets.iter().any(|a| a.name == "anyr-linux-x86_64"),
            "{beta:?}"
        );
        assert!(beta.assets.iter().any(|a| a.browser_download_url
            == "https://github.com/AnyRouterHQ/cli/releases/download/v0.1.12-beta.98/anyr-linux-x86_64"));
    }

    #[test]
    fn parse_releases_html_handles_transferred_owner() {
        // WHY: the repo moved anyrouter-dev/cli -> AnyRouterHQ/cli; the
        // live page now uses /AnyRouterHQ/cli/releases/tag/... and the old
        // owner-anchored parser returned zero tags.
        let html = r#"
<a href="/AnyRouterHQ/cli/releases/tag/v0.1.37">v0.1.37</a>
<span>Latest</span>
<a href="/AnyRouterHQ/cli/releases/tag/v0.1.38-beta.689">v0.1.38-beta.689</a>
<span class="Label">Pre-release</span>
<a href="/AnyRouterHQ/cli/releases/download/v0.1.37/anyr-linux-x86_64">anyr-linux-x86_64</a>
"#;
        let rels = parse_releases_html(html).unwrap();
        assert_eq!(rels.len(), 2, "{rels:?}");
        let stable = rels.iter().find(|r| r.tag_name == "v0.1.37").unwrap();
        assert!(!stable.prerelease);
        assert!(
            stable.assets.iter().any(|a| a.name == "anyr-linux-x86_64"),
            "{stable:?}"
        );
        let beta = rels
            .iter()
            .find(|r| r.tag_name == "v0.1.38-beta.689")
            .unwrap();
        assert!(beta.prerelease);
    }

    #[test]
    fn parse_expanded_assets_html_lists_linux_x86_64() {
        let html = r#"<a href="/anyrouter-dev/cli/releases/download/v0.1.12-beta.98/anyr-linux-x86_64">anyr-linux-x86_64</a>"#;
        let mut rel = Release {
            tag_name: "v0.1.12-beta.98".into(),
            prerelease: true,
            assets: Vec::new(),
        };
        merge_expanded_assets(&mut rel, html);
        assert_eq!(rel.assets.len(), 1);
        assert_eq!(rel.assets[0].name, "anyr-linux-x86_64");
    }

    const EMPTY_STABLE: &str = r#"[
  {"tag_name":"v0.1.11","prerelease":false,"assets":[]},
  {"tag_name":"v0.1.12-beta.98","prerelease":true,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/anyrouter-dev/cli/releases/download/v0.1.12-beta.98/anyr-linux-x86_64"}]}
]"#;

    #[test]
    fn channel_update_candidates_newest_first_skips_empty() {
        let json = r#"[
  {"tag_name":"v0.1.14","prerelease":false,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://example/0.1.14"}]},
  {"tag_name":"v0.1.13","prerelease":false,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://example/0.1.13"}]},
  {"tag_name":"v0.1.12","prerelease":false,"assets":[]},
  {"tag_name":"v0.1.15-beta.1","prerelease":true,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://example/beta"}]}
]"#;
        let rels = parse_releases(json).unwrap();
        let tags: Vec<_> = channel_update_candidates(&rels, Channel::Stable, "anyr-linux-x86_64")
            .into_iter()
            .map(|r| r.tag_name)
            .collect();
        assert_eq!(tags, vec!["v0.1.14".to_string(), "v0.1.13".to_string()]);
    }

    #[test]
    fn select_latest_with_asset_skips_empty_stable() {
        let rels = parse_releases(EMPTY_STABLE).unwrap();
        let err = select_latest_release_with_asset(&rels, Channel::Stable, "anyr-linux-x86_64")
            .unwrap_err();
        assert!(err.contains("anyr-linux-x86_64"), "{err}");
        assert!(err.contains("update --beta"), "{err}");
        assert!(!err.contains("GitHub Releases API HTTP 403"), "{err}");
        let beta =
            select_latest_release_with_asset(&rels, Channel::Beta, "anyr-linux-x86_64").unwrap();
        assert_eq!(beta.tag_name, "v0.1.12-beta.98");
    }

    #[test]
    fn musl_assets_only_on_linux_and_match_the_running_build() {
        assert_eq!(
            asset_name_for("linux", "x86_64", true),
            "anyr-linux-x86_64-musl"
        );
        assert_eq!(
            asset_name_for("linux", "arm64", true),
            "anyr-linux-arm64-musl"
        );
        assert_eq!(asset_name_for("linux", "arm64", false), "anyr-linux-arm64");
        assert_eq!(asset_name_for("darwin", "arm64", true), "anyr-darwin-arm64");
        // WHY: the updater must fetch the variant it is, or a musl install on
        // Alpine would update itself to a glibc binary that cannot start.
        assert_eq!(
            asset_name("linux", "x86_64").ends_with("-musl"),
            cfg!(target_env = "musl")
        );
    }

    #[test]
    fn musl_build_never_falls_back_to_glibc_only_releases() {
        // WHY: releases before the musl build list only glibc assets; the
        // any-assets fallback would pick them and install a binary that
        // cannot run on a musl system.
        let json = r#"[
          {"tag_name":"v0.1.44","prerelease":false,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/AnyRouterHQ/cli/releases/download/v0.1.44/anyr-linux-x86_64"}]}
        ]"#;
        let rels = parse_releases(json).unwrap();
        let musl = "anyr-linux-x86_64-musl";
        for channel in [Channel::Stable, Channel::Beta] {
            assert!(channel_update_candidates(&rels, channel, musl).is_empty());
            let err = select_latest_release_with_asset(&rels, channel, musl).unwrap_err();
            assert!(err.contains(musl) && err.contains("glibc"), "{err}");
        }
        assert_eq!(
            channel_update_candidates(&rels, Channel::Stable, "anyr-linux-arm64").len(),
            1,
            "glibc builds keep the any-assets fallback"
        );
    }

    #[test]
    fn beta_channel_takes_a_stable_cut_after_the_last_beta() {
        let json = r#"[
          {"tag_name":"v0.1.21-beta.255","prerelease":true,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/anyrouter-dev/cli/releases/download/v0.1.21-beta.255/anyr-linux-x86_64"}]},
          {"tag_name":"v0.1.21","prerelease":false,"assets":[{"name":"anyr-linux-x86_64","browser_download_url":"https://github.com/anyrouter-dev/cli/releases/download/v0.1.21/anyr-linux-x86_64"}]}
        ]"#;
        assert_eq!(
            select_latest(json, Channel::Beta).unwrap().tag_name,
            "v0.1.21"
        );
        let rels = parse_releases(json).unwrap();
        let tags: Vec<String> =
            channel_update_candidates(&rels, Channel::Beta, "anyr-linux-x86_64")
                .into_iter()
                .map(|r| r.tag_name)
                .collect();
        assert_eq!(tags, vec!["v0.1.21", "v0.1.21-beta.255"]);
    }
}
