import { describe, expect, it } from "vitest";
import { stampStyleNonce } from "../src/cspNonce";

function page(nonce: string | null): Document {
  const doc = document.implementation.createHTMLDocument("t");
  if (nonce !== null) {
    const meta = doc.createElement("meta");
    meta.setAttribute("property", "csp-nonce");
    meta.nonce = nonce;
    doc.head.append(meta);
  }
  return doc;
}

describe("stampStyleNonce", () => {
  it("gives every style element the page's nonce and nothing else one", () => {
    const doc = page("bm9uY2Vub25jZW5vbmNl");
    stampStyleNonce(doc);
    expect(doc.createElement("style").nonce).toBe("bm9uY2Vub25jZW5vbmNl");
    expect(doc.createElement("STYLE").nonce).toBe("bm9uY2Vub25jZW5vbmNl");
    expect(doc.createElement("script").nonce).toBe("");
    expect(doc.createElement("div").nonce).toBe("");
  });

  it("stamps nothing without a nonce or with the build's placeholder", () => {
    for (const doc of [page(null), page("__JC_CSP_NONCE__")]) {
      stampStyleNonce(doc);
      expect(doc.createElement("style").nonce).toBe("");
    }
  });
});
