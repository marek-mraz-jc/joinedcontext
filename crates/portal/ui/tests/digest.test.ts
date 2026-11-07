/**
 * The client hashes a manifest the way the Portal does (T-0844, PF-57): the form compares the
 * verdict's `inputDigest` against this value, so a difference would make every green verdict stale.
 */
import { describe, expect, it } from "vitest";
import { canonicalizeJson, digestOf, sha256Hex } from "../src/api/digest";

describe("the draft digest", () => {
  it("hashes the empty input and one known string as FIPS 180-4 says", () => {
    expect(sha256Hex(new Uint8Array())).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
    expect(sha256Hex(new TextEncoder().encode("abc"))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });

  it("hashes a message longer than one block and one that fills a block exactly", () => {
    expect(sha256Hex(new TextEncoder().encode("a".repeat(55)))).toBe(
      "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
    );
    expect(sha256Hex(new TextEncoder().encode("a".repeat(64)))).toBe(
      "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
    );
  });

  it("is the value the Portal writes for the same manifest", () => {
    // The canonical form is `{"a":1,"b":"x"}`, the document `src/ops/verdict.rs` records.
    expect(digestOf({ b: "x", a: 1 })).toBe(
      "sha256:ecf9e98ec0641e23113ff3ce8bdc78d0ddd249886517fd4a7f68cc83d4e65667",
    );
  });

  it("does not depend on the order the keys were typed in, and changes with the content", () => {
    const a = { name: "foo", spec: { url: "https://example.com", port: 80 } };
    const b = { spec: { port: 80, url: "https://example.com" }, name: "foo" };
    expect(digestOf(a)).toBe(digestOf(b));
    expect(digestOf({ ...a, name: "bar" })).not.toBe(digestOf(a));
    expect(canonicalizeJson(b)).toEqual(canonicalizeJson(a));
  });

  it("hashes text outside ASCII by its UTF-8 bytes", () => {
    expect(digestOf({ title: "Ähre" })).toBe(
      `sha256:${sha256Hex(new TextEncoder().encode('{"title":"Ähre"}'))}`,
    );
  });
});
