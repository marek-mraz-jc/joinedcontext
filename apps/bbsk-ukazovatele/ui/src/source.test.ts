/**
 * What the bundle is allowed to contain (AP-23, AP-49, T-2308 security).
 *
 * CI enforces these at the network level for every app; this file is the same rules where a
 * person can read them, and it fails in seconds instead of at publication. The application holds
 * no credential and speaks to no host: the reader arrives already authenticated and the endpoint
 * is reached on the origin that served the page.
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const HERE = join(process.cwd(), "src");

function sources(dir = HERE): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === "fixtures" ? [] : sources(path);
    return /\.tsx?$/.test(entry.name) && !entry.name.endsWith(".test.ts") && !entry.name.endsWith(".test.tsx")
      ? [path]
      : [];
  });
}

const FILES = sources().map((path) => [path.slice(HERE.length + 1), readFileSync(path, "utf8")] as const);

describe("what the application is allowed to contain", () => {
  it("has sources to check", () => {
    expect(FILES.length).toBeGreaterThan(3);
  });

  it("reaches no host of its own", () => {
    for (const [name, text] of FILES) {
      // Every read goes through the SDK, which builds `/api/endpoint/{slug}/…` on this origin.
      expect(text, name).not.toMatch(/https?:\/\/(?!www\.w3\.org)/);
      expect(text, name).not.toMatch(/\bfetch\s*\(/);
      expect(text, name).not.toMatch(/XMLHttpRequest|EventSource|WebSocket/);
    }
  });

  it("handles no login, session or token", () => {
    for (const [name, text] of FILES) {
      expect(text, name).not.toMatch(/\b(oidc|Bearer|access_token|refresh_token|client_secret)\b/i);
      expect(text, name).not.toMatch(/\b(localStorage|sessionStorage|document\.cookie)\b/);
    }
  });

  it("writes nothing", () => {
    for (const [name, text] of FILES) {
      expect(text, name).not.toMatch(/\.(patch|remove|create|update)\s*\(/);
      expect(text, name).not.toMatch(/method:\s*["'](POST|PATCH|PUT|DELETE)/);
    }
  });

  it("carries no escape from the type system and nothing left unfinished", () => {
    for (const [name, text] of FILES) {
      expect(text, name).not.toMatch(/\bas any\b|\bany\[\]|:\s*any\b/);
      expect(text, name).not.toMatch(/@ts-(ignore|expect-error|nocheck)/);
      expect(text, name).not.toMatch(/\bTODO\b|\bFIXME\b/);
    }
  });

  it("puts no visible string in a component", () => {
    // Everything a reader sees comes from `locales.ts`; a sentence in a component would be one
    // language only, and Slovak is the language of both publishers.
    const components = FILES.filter(([name]) => name.endsWith(".tsx"));
    expect(components.length).toBeGreaterThan(0);
    for (const [name, text] of components) {
      const jsxText = text.match(/>[^<>{}\n]*[A-Za-zÁ-ž]{4,}[^<>{}\n]*</g) ?? [];
      expect(jsxText, name).toEqual([]);
    }
  });
});
