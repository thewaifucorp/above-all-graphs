// Postinstall: download the prebuilt `aag` binary for this platform from
// GitHub Releases. No compile step, no toolchain, no onnxruntime — that
// friction is exactly what aag exists to eliminate (SPEC.md section 0).
//
// The release tag is pinned to this package's version, so npm version N
// always fetches binary version N.

"use strict";

const fs = require("node:fs");
const path = require("node:path");
const zlib = require("node:zlib");
const { pipeline } = require("node:stream/promises");

const REPO = "thewaifucorp/above-all-graphs";
const VERSION = require("./package.json").version;

// Semantic search (local embeddings) ships as a separate, larger asset: the
// same binary built with `--features semantic`, with onnxruntime linked
// statically so it stays one self-contained file. Opt in with
// `AAG_SEMANTIC=1 npm i -g @waifucorp/aag` or `npm i --aag-semantic`.
function wantsSemantic(env = process.env) {
  const flags = [env.AAG_SEMANTIC, env.npm_config_aag_semantic];
  return flags.some((value) =>
    ["1", "true", "yes", "on"].includes(String(value ?? "").toLowerCase()),
  );
}

// The release asset for a platform key, e.g. `linux-x64` -> the tar.gz name.
// `null` when the platform has no prebuilt binary.
function assetFor(key, semantic) {
  const target = TARGETS[key];
  if (!target) return null;
  const windows = key.startsWith("win32");
  const variant = semantic ? "aag-semantic" : "aag";
  return `${variant}-${target}.${windows ? "zip" : "tar.gz"}`;
}

const TARGETS = {
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
};

async function main() {
  const key = `${process.platform}-${process.arch}`;
  const semantic = wantsSemantic();
  const asset = assetFor(key, semantic);
  if (!asset) {
    console.error(`aag: unsupported platform ${key}`);
    process.exit(1);
  }

  const windows = process.platform === "win32";
  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${asset}`;
  if (semantic) {
    console.log("aag: AAG_SEMANTIC set — installing the build with local embeddings");
  }
  const binDir = path.join(__dirname, "bin");
  const binPath = path.join(binDir, windows ? "aag.exe" : "aag");
  fs.mkdirSync(binDir, { recursive: true });

  console.log(`aag: downloading ${url}`);
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) {
    console.error(`aag: download failed (${response.status}) — ${url}`);
    if (semantic) {
      console.error(
        "aag: the semantic build may not exist for this release; " +
          "install without AAG_SEMANTIC to get the standard binary.",
      );
    }
    process.exit(1);
  }

  if (windows) {
    // Node has no built-in zip reader; buffer the archive and shell out to
    // PowerShell's Expand-Archive (present on every supported Windows).
    const zipPath = path.join(binDir, asset);
    fs.writeFileSync(zipPath, Buffer.from(await response.arrayBuffer()));
    const { execFileSync } = require("node:child_process");
    execFileSync("powershell.exe", [
      "-NoProfile",
      "-Command",
      `Expand-Archive -Force -Path '${zipPath}' -DestinationPath '${binDir}'`,
    ]);
    fs.unlinkSync(zipPath);
  } else {
    // tar.gz with a single file: gunzip via zlib, then a minimal tar read
    // (header name + size), avoiding a runtime dependency on `tar`.
    const gzPath = path.join(binDir, asset);
    await pipeline(response.body, fs.createWriteStream(gzPath));
    const tarBuffer = zlib.gunzipSync(fs.readFileSync(gzPath));
    fs.unlinkSync(gzPath);
    extractSingleFileTar(tarBuffer, "aag", binPath);
    fs.chmodSync(binPath, 0o755);
  }

  console.log(`aag: installed ${binPath}`);
}

// Reads a POSIX tar archive and writes the entry named `wanted` to `out`.
function extractSingleFileTar(buffer, wanted, out) {
  let offset = 0;
  while (offset + 512 <= buffer.length) {
    const name = buffer
      .subarray(offset, offset + 100)
      .toString("utf8")
      .replace(/\0.*$/, "");
    if (!name) break;
    const size = parseInt(
      buffer.subarray(offset + 124, offset + 136).toString("utf8").trim(),
      8,
    );
    const start = offset + 512;
    if (name === wanted || name.endsWith(`/${wanted}`)) {
      fs.writeFileSync(out, buffer.subarray(start, start + size));
      return;
    }
    offset = start + Math.ceil(size / 512) * 512;
  }
  console.error(`aag: '${wanted}' not found in archive`);
  process.exit(1);
}

// Exported for the installer's own tests; `require.main` guards the run so
// importing this file never downloads anything.
module.exports = { assetFor, wantsSemantic };

if (require.main !== module) {
  return;
}

main().catch((error) => {
  console.error(`aag: install failed — ${error.message}`);
  process.exit(1);
});
