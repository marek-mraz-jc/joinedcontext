// @vitest-environment node
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";
import { afterAll, describe, expect, it } from "vitest";

const CONFIG = fileURLToPath(new URL("../vite.template.config.ts", import.meta.url));
const out = mkdtempSync(join(tmpdir(), "jc-template-build-"));

afterAll(() => rmSync(out, { recursive: true, force: true }));

describe("the template's build", () => {
  // SDK-19, AP-80, T-2602: MapLibre asks for its worker beside its own chunk, which a build does
  // not have, so a built application's map never drew. The worker is in the bundle, and the page
  // loads the module that names it before the application starts.
  it("carries MapLibre's worker and loads it before the application", async () => {
    await build({ configFile: CONFIG, logLevel: "silent", build: { outDir: out, emptyOutDir: true } });

    const assets = readdirSync(join(out, "assets"));
    const worker = assets.find((name) => /^maplibre-gl-worker-.*\.js$/.test(name));
    expect(worker, assets.join(", ")).toBeDefined();
    const code = readFileSync(join(out, "assets", worker!), "utf8");
    expect(code).not.toMatch(/from\s*["'][^"']*maplibre-gl-shared/);

    const html = readFileSync(join(out, "index.html"), "utf8");
    const scripts = [...html.matchAll(/<script[^>]*type="module"[^>]*src="([^"]+)"/g)].map((m) => m[1]);
    expect(scripts.length).toBeGreaterThan(0);
    const bundled = scripts.map((src) => readFileSync(join(out, src.replace(/^\.?\//, "")), "utf8")).join("\n");
    expect(bundled).toContain(worker!);
  }, 60_000);
});
