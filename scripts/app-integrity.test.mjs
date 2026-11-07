// node --test scripts/app-integrity.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { digests, sri } from "./app-integrity.mjs";

test("the digest is the one the static host computes", () => {
  // The same constant src/apps/static_host.rs asserts for its sri_sha384 of the empty input.
  assert.equal(sri(Buffer.alloc(0)), "sha384-OLBgp1GsljhM2TJ+sbHjaiH9txEUvgdDTAzHv2P24donTt6/529l+9Ua0vFImLlb");
});

test("every file is recorded by its bundle-relative path, the manifest itself is not", () => {
  const root = mkdtempSync(join(tmpdir(), "jc-integrity-"));
  try {
    mkdirSync(join(root, "assets"));
    writeFileSync(join(root, "index.html"), "<!doctype html>");
    writeFileSync(join(root, "assets", "index.js"), "console.log(1)");
    writeFileSync(join(root, "integrity.json"), "{}");
    const map = digests(root);
    assert.deepEqual(Object.keys(map), ["assets/index.js", "index.html"]);
    assert.equal(map["index.html"], sri(Buffer.from("<!doctype html>")));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a bundle without an index is refused", () => {
  const root = mkdtempSync(join(tmpdir(), "jc-integrity-"));
  try {
    writeFileSync(join(root, "app.js"), "");
    assert.throws(() => digests(root), /no index\.html/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
