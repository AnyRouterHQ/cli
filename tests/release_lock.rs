//! Static assertions that this repo stays on 0.1.x and ships release files.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn quoted_after_key(text: &str, key: &str) -> String {
    let mut search = text;
    while let Some(idx) = search.find(key) {
        let after = search[idx + key.len()..].trim_start();
        let after = after
            .strip_prefix(':')
            .or_else(|| after.strip_prefix('='))
            .unwrap_or(after)
            .trim_start();
        if let Some(rest) = after.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                return rest[..end].to_string();
            }
        }
        search = &search[idx + key.len()..];
    }
    panic!("did not find quoted value for {key:?}");
}

fn cargo_package_version(toml: &str) -> String {
    let mut in_package = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
            continue;
        }
        if in_package && t.starts_with("version") {
            return quoted_after_key(t, "version");
        }
    }
    panic!("Cargo.toml [package] version missing");
}

fn assert_0_1(label: &str, version: &str) {
    assert!(
        version.starts_with("0.1."),
        "{label} version {version:?} does not match ^0\\.1\\."
    );
}

fn must_exist(rel: &str) {
    let path = root().join(rel);
    assert!(path.is_file(), "missing required file {}", path.display());
}

fn contains_true(hay: &str, key: &str) {
    let compact = hay.replace([' ', '\n', '\t', '\r'], "");
    let needle = format!("\"{key}\":true");
    assert!(
        compact.contains(&needle),
        "expected {key} = true in config, compact={compact}"
    );
}

#[test]
fn versions_are_0_1_x() {
    let cargo = cargo_package_version(&read("Cargo.toml"));
    let pkg = quoted_after_key(&read("package.json"), "\"version\"");
    let manifest = quoted_after_key(&read(".release-please-manifest.json"), "\".\"");
    let lib = read("src/lib.rs");

    assert_0_1("Cargo.toml", &cargo);
    assert_0_1("package.json", &pkg);
    assert_0_1(".release-please-manifest.json", &manifest);
    assert_eq!(
        cargo, pkg,
        "Cargo.toml and package.json versions must match"
    );
    assert_eq!(
        cargo, manifest,
        "Cargo.toml and release-please manifest versions must match"
    );
    assert!(
        lib.contains("env!(\"CARGO_PKG_VERSION\")"),
        "lib.rs VERSION must follow Cargo.toml"
    );
}

#[test]
fn release_please_manifest_has_no_0_2() {
    let manifest = read(".release-please-manifest.json");
    assert!(
        !manifest.contains("\"version\": \"0.2"),
        "release-please manifest must not contain \"version\": \"0.2\": {manifest}"
    );
    assert!(
        !manifest.contains("\"0.2"),
        "release-please manifest must stay on 0.1.x: {manifest}"
    );
}

#[test]
fn release_please_always_bumps_patch() {
    let config = read("release-please-config.json");
    let compact = config.replace([' ', '\n', '\t', '\r'], "");
    assert!(
        compact.contains("\"versioning\":\"always-bump-patch\""),
        "versioning must be always-bump-patch so tagged releases stay 0.1.x"
    );
    assert!(
        !compact.contains("\"bump-minor-pre-major\":true"),
        "bump-minor-pre-major:true is a 0.2.0 path (feat! / BREAKING CHANGE)"
    );
    contains_true(&config, "include-v-in-tag");
    assert!(
        config.contains("\"@anyr/cli\""),
        "package-name should be @anyr/cli"
    );
    assert!(
        config.contains("Cargo.toml"),
        "extra-files must include Cargo.toml"
    );
    assert!(
        compact.contains("\"release-type\":\"node\""),
        "node release-type updates CHANGELOG + package.json"
    );
}

#[test]
fn workflow_files_exist_and_do_not_auto_merge() {
    let workflows = [
        ".github/workflows/ci.yml",
        ".github/workflows/release-please.yml",
        ".github/workflows/release-binaries.yml",
        ".github/workflows/npm-publish.yml",
    ];
    for rel in workflows {
        must_exist(rel);
        let body = read(rel);
        assert!(
            !body.contains("gh pr merge"),
            "{rel} must not auto-merge (found gh pr merge)"
        );
        assert!(
            !body.contains("--auto"),
            "{rel} must not auto-merge (found --auto)"
        );
    }

    let binaries = read(".github/workflows/release-binaries.yml");
    for asset in [
        "anyr-linux-x86_64",
        "anyr-linux-arm64",
        "anyr-darwin-x86_64",
        "anyr-darwin-arm64",
        "anyr-windows-x86_64.exe",
        "anyr.wasm",
    ] {
        assert!(
            binaries.contains(asset),
            "release-binaries must ship {asset}"
        );
    }
    assert!(
        !binaries.contains("optional: true"),
        "platform builds must not be optional"
    );
    assert!(
        binaries.contains("macos-13"),
        "stable releases still ship Intel macOS via macos-13"
    );
    assert!(
        binaries.contains("checksums:"),
        "release-binaries must have a dedicated checksums job"
    );
    assert!(
        binaries.contains("always()"),
        "checksums must run even if a matrix platform fails"
    );
    assert!(
        binaries.contains("release not found"),
        "uploads must retry release-not-found inline (historical tags lack helper scripts)"
    );
    assert!(
        !binaries.contains("bash scripts/"),
        "workflow must not call helper scripts from a tag checkout"
    );
    assert!(
        binaries.contains("hard gate"),
        "checksums job must fail when the release has zero binaries"
    );
    assert!(
        binaries.contains("ref: ${{ github.sha }}"),
        "checksums/notes must check out the workflow SHA, not the historical tag"
    );
    assert!(
        binaries.contains("bench-report.md"),
        "release notes must include the bench report"
    );
    assert!(
        binaries.contains("scripts/release_notes.py github"),
        "release notes must use the release-please changelog"
    );
    assert!(
        !binaries.contains("gh release view"),
        "release notes must not keep a short handwritten body"
    );

    let ci = read(".github/workflows/ci.yml");
    for needle in [
        "macos-latest",
        "ubuntu-latest",
        "ubuntu-24.04-arm",
        "windows-latest",
        "wasm32-unknown-unknown",
        "scripts/bench.py",
        "anyr-bench-report",
        "cargo nextest run --locked --all-targets",
        "cargo llvm-cov nextest --locked --all-targets",
        "codecov/codecov-action",
        "lcov.info",
        "test -s dist/checksums.txt",
    ] {
        assert!(ci.contains(needle), "ci.yml must include {needle}");
    }
    must_exist("codecov.yml");
    let codecov = read("codecov.yml");
    assert!(
        codecov.contains("coverage:"),
        "codecov.yml must configure coverage status"
    );

    let ci_body = read(".github/workflows/ci.yml");
    assert!(
        !ci_body.contains("actions/deploy-pages"),
        "ci.yml must not deploy GitHub Pages"
    );
    assert!(
        !ci_body.contains("wasm-demo"),
        "ci.yml must not upload a GH Pages wasm demo"
    );
    let binaries_body = read(".github/workflows/release-binaries.yml");
    assert!(
        !binaries_body.contains("anyr-wasm-demo.tar.gz"),
        "releases must not ship a GH Pages wasm demo tarball"
    );
    assert!(
        !root().join(".github/workflows/pages.yml").exists(),
        "GitHub Pages workflow must not exist"
    );
    assert!(
        !root().join("web/index.html").exists(),
        "GH Pages wasm playground must not exist"
    );

    let npm = read(".github/workflows/npm-publish.yml");
    assert!(
        npm.contains("npm publish --access public --tag next"),
        "npm must publish to the next tag"
    );

    let rp = read(".github/workflows/release-please.yml");
    assert!(
        rp.contains("googleapis/release-please-action"),
        "release-please workflow must use googleapis/release-please-action"
    );
}

#[test]
fn npm_package_is_public_next_tag() {
    let pkg = read("package.json");
    let compact = pkg.replace([' ', '\n', '\t', '\r'], "");
    assert!(compact.contains("\"access\":\"public\""));
    assert!(compact.contains("\"tag\":\"next\""));
    assert!(compact.contains("\"cli\":\"scripts/npx-anyr.js\""));
    assert!(compact.contains("\"anyr\":\"scripts/npx-anyr.js\""));
    assert!(compact.contains("\"anyrouter\":\"scripts/npx-anyr.js\""));
    assert!(compact.contains("\"ar\":\"scripts/npx-anyr.js\""));
    assert!(pkg.contains("git+https://github.com/AnyRouterHQ/cli.git"));
}

#[test]
fn wrapper_and_install_scripts_exist() {
    must_exist("scripts/npx-anyr.js");
    must_exist("scripts/install.sh");
    must_exist("scripts/install-hooks.sh");
    must_exist("scripts/check-coauthors.sh");
    must_exist("scripts/bench.py");
    must_exist("scripts/release_notes.py");
    must_exist("tests/release_notes_test.py");
    let notes = read("scripts/release_notes.py");
    assert!(
        notes.contains("release-please"),
        "release_notes.py must build release-please changelog notes"
    );
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("tests/release_notes_test.py"),
        "ci.yml must run release notes tests"
    );
    let bench = read("scripts/bench.py");
    assert!(
        bench.contains("def measure"),
        "bench.py must measure size/startup"
    );
    assert!(
        bench.contains("def report"),
        "bench.py must fold JSON into markdown"
    );
    must_exist("LICENSE");
    must_exist("CHANGELOG.md");
    must_exist("README.md");
    let wrapper = read("scripts/npx-anyr.js");
    assert!(wrapper.contains("github.com/AnyRouterHQ/cli"));
    assert!(wrapper.contains("binaries"));
    assert!(wrapper.contains("anyr-windows-x86_64.exe"));
    let install = read("scripts/install.sh");
    assert!(install.contains("--channel"));
    assert!(install.contains("stable"));
    assert!(install.contains("beta"));
}

#[test]
fn hook_source_contains_both_coauthor_emails() {
    let hook = Path::new(".githooks").join("prepare-commit-msg");
    let body = read(hook.to_str().unwrap());
    assert!(
        body.contains("Co-authored-by: Duyet Le <me@duyet.net>"),
        "missing Duyet Le trailer"
    );
    assert!(
        body.contains("Co-authored-by: duyetbot <bot@duyet.net>"),
        "missing duyetbot trailer"
    );
    let check = read("scripts/check-coauthors.sh");
    assert!(check.contains("me@duyet.net"));
    assert!(check.contains("bot@duyet.net"));
}
