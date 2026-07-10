#!/usr/bin/env node
// Thin shim: exec the real binary that install.js placed next to this
// file, forwarding args, stdio, and exit code untouched — MCP servers
// speak over stdio, so the shim must not buffer or transform anything.

"use strict";

const path = require("node:path");
const { spawnSync } = require("node:child_process");

const binary = path.join(
  __dirname,
  process.platform === "win32" ? "aag.exe" : "aag",
);

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(
    `aag: binary missing or broken (${result.error.message}) — try reinstalling: npm i -g @waifucorp/aag`,
  );
  process.exit(1);
}
process.exit(result.status ?? 0);
