// The installer picks one of two release assets per platform. Getting that
// wrong means a user who asked for embeddings silently gets the build without
// them, so the mapping is asserted rather than assumed.
//
// Run with `node --test npm/`.

"use strict";

const test = require("node:test");
const assert = require("node:assert");

const { assetFor, wantsSemantic } = require("./install.js");

test("each supported platform maps to its release asset", () => {
  assert.strictEqual(
    assetFor("linux-x64", false),
    "aag-x86_64-unknown-linux-gnu.tar.gz",
  );
  assert.strictEqual(
    assetFor("darwin-arm64", false),
    "aag-aarch64-apple-darwin.tar.gz",
  );
  assert.strictEqual(
    assetFor("win32-x64", false),
    "aag-x86_64-pc-windows-msvc.zip",
  );
});

test("the semantic opt-in selects the semantic asset, same platform", () => {
  assert.strictEqual(
    assetFor("linux-arm64", true),
    "aag-semantic-aarch64-unknown-linux-gnu.tar.gz",
  );
  assert.strictEqual(
    assetFor("win32-x64", true),
    "aag-semantic-x86_64-pc-windows-msvc.zip",
  );
});

test("an unsupported platform has no asset rather than a wrong one", () => {
  assert.strictEqual(assetFor("sunos-sparc", false), null);
  assert.strictEqual(assetFor("linux-mips", true), null);
});

test("semantic is opt-in, from either the env var or the npm flag", () => {
  assert.strictEqual(wantsSemantic({ AAG_SEMANTIC: "1" }), true);
  assert.strictEqual(wantsSemantic({ AAG_SEMANTIC: "true" }), true);
  assert.strictEqual(wantsSemantic({ npm_config_aag_semantic: "yes" }), true);
  assert.strictEqual(wantsSemantic({}), false);
  assert.strictEqual(wantsSemantic({ AAG_SEMANTIC: "0" }), false);
  assert.strictEqual(wantsSemantic({ AAG_SEMANTIC: "" }), false);
});
