fn main() {
    println!("cargo:rustc-env=ANYR_BUILD_TIME_UTC={}", utc_now());
    println!("cargo:rustc-check-cfg=cfg(anyr_foundation_model)");
    println!("cargo:rerun-if-changed=src/foundation_relay.swift");
    compile_foundation_helper();
}

/// Embed the system-model helper on Apple Silicon only. The dylib is bytes
/// inside `anyr`, loaded at runtime when the OS can run it. Intel, Windows,
/// and Linux builds do not compile or advertise it.
///
/// A missing toolchain or a helper that will not compile must not fail the
/// build: warn and build without the helper. The relay then keeps the HTTP
/// (`fm serve`) path and never mentions the in-process model.
fn compile_foundation_helper() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target != "aarch64-apple-darwin" {
        return;
    }
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let dylib = std::path::Path::new(&out_dir).join("libanyrfm.dylib");
    let sdk = match xcrun(&["--sdk", "macosx", "--show-sdk-path"]) {
        Ok(sdk) => sdk,
        Err(err) => {
            println!("cargo:warning=building without the system model helper: {err}");
            return;
        }
    };
    if sdk.is_empty() {
        println!("cargo:warning=building without the system model helper: macOS SDK path is empty");
        return;
    }
    let mut cmd = std::process::Command::new("swiftc");
    cmd.args([
        "-emit-library",
        "-parse-as-library",
        "-swift-version",
        "5",
        "-module-name",
        "AnyrFm",
        "-target",
        "arm64-apple-macosx26.0",
        "-sdk",
        sdk.trim(),
        "-o",
    ])
    .arg(&dylib)
    .arg("src/foundation_relay.swift")
    .args([
        "-framework",
        "Foundation",
        "-Xlinker",
        "-weak_framework",
        "-Xlinker",
        "FoundationModels",
        "-Xlinker",
        "-rpath",
        "-Xlinker",
        "/usr/lib/swift",
    ]);
    if std::env::var("PROFILE").ok().as_deref() == Some("release") {
        cmd.arg("-O");
    }
    let output = match cmd.output() {
        Ok(output) => output,
        Err(err) => {
            println!(
                "cargo:warning=building without the system model helper: could not run swiftc: {err}"
            );
            return;
        }
    };
    if !output.status.success() {
        println!(
            "cargo:warning=building without the system model helper: swiftc failed\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    println!("cargo:rustc-cfg=anyr_foundation_model");
}

fn xcrun(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("xcrun")
        .args(args)
        .output()
        .map_err(|err| format!("could not run xcrun: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "xcrun {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Build time as `YYYY-MM-DDTHH:MM:SSZ` (UTC), std-only — civil-from-days per
/// Howard Hinnant's algorithm. Stored in UTC; the CLI renders it in the
/// viewer's local timezone at runtime.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Split a unix timestamp into `(year, month, day, hour, minute, second)` in UTC.
pub fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Shift the civil epoch so March is month 3; the year then starts in March.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo as u32, d as u32, h as u32, mi as u32, s as u32)
}
