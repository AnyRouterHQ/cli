#!/usr/bin/env node
"use strict";

const { spawnSync } = require("child_process");
const fs = require("fs");
const https = require("https");
const os = require("os");
const path = require("path");

const VERSION = require("../package.json").version;
const GITHUB_DOWNLOAD = "https://github.com/AnyRouterHQ/cli/releases/download";

function assetName() {
  const plat = process.platform;
  const arch = process.arch;
  if (plat === "linux" && arch === "x64") return "anyr-linux-x86_64";
  if (plat === "linux" && arch === "arm64") return "anyr-linux-arm64";
  if (plat === "darwin" && arch === "x64") return "anyr-darwin-x86_64";
  if (plat === "darwin" && arch === "arm64") return "anyr-darwin-arm64";
  if (plat === "win32" && arch === "x64") return "anyr-windows-x86_64.exe";
  console.error(`Unsupported platform: ${plat}-${arch}`);
  process.exit(1);
}

function shipped(name) {
  const dir = path.join(__dirname, "..", "binaries");
  for (const candidate of [name, "anyr"]) {
    const p = path.join(dir, candidate);
    try {
      fs.accessSync(p, fs.constants.X_OK);
      return p;
    } catch {
      /* missing */
    }
  }
  return null;
}

function cachePath(name) {
  const dir = path.join(os.homedir(), ".anyrouter", "binaries", VERSION);
  fs.mkdirSync(dir, { recursive: true });
  return path.join(dir, name);
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const tmp = `${dest}.tmp`;
    const follow = (u) => {
      https
        .get(u, { headers: { "User-Agent": "anyr-cli" } }, (res) => {
          if (
            res.statusCode >= 300 &&
            res.statusCode < 400 &&
            res.headers.location
          ) {
            res.resume();
            follow(res.headers.location);
            return;
          }
          if (res.statusCode !== 200) {
            res.resume();
            reject(new Error(`download failed: HTTP ${res.statusCode} for ${u}`));
            return;
          }
          const out = fs.createWriteStream(tmp, { mode: 0o755 });
          res.pipe(out);
          out.on("finish", () => {
            out.close(() => {
              try {
                fs.chmodSync(tmp, 0o755);
                fs.renameSync(tmp, dest);
                resolve();
              } catch (err) {
                reject(err);
              }
            });
          });
          out.on("error", reject);
        })
        .on("error", reject);
    };
    follow(url);
  });
}

async function resolveBin() {
  if (process.env.ANYR_BIN) return process.env.ANYR_BIN;
  const name = assetName();
  const local = shipped(name);
  if (local) return local;
  const cached = cachePath(name);
  if (fs.existsSync(cached)) return cached;
  const url = `${GITHUB_DOWNLOAD}/v${VERSION}/${name}`;
  process.stderr.write(`Downloading ${name} v${VERSION} from GitHub Releases…\n`);
  try {
    await download(url, cached);
  } catch (err) {
    throw new Error(
      `${err.message}\nFailed to fetch ${url}. Build from source: cargo install --path . --locked`
    );
  }
  return cached;
}

function displayBin() {
  if (process.env.ANYR_DISPLAY_BIN) return process.env.ANYR_DISPLAY_BIN;
  const invoked = path
    .basename(process.argv[1] || "")
    .replace(/\.js$/i, "");
  if (invoked === "ar") return "ar";
  if (invoked === "anyrouter") return "anyrouter";
  if (invoked === "anyr") return "anyr";
  // `npx @anyr/cli` resolves the `cli` bin; the shim file is npx-anyr.js.
  return "npx @anyr/cli";
}

async function main() {
  const bin = await resolveBin();
  const env = { ...process.env, ANYR_DISPLAY_BIN: displayBin() };
  const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit", env });
  if (result.error) {
    console.error(result.error.message);
    process.exit(1);
  }
  process.exit(result.status == null ? 1 : result.status);
}

main().catch((err) => {
  console.error(err.message || err);
  process.exit(1);
});
