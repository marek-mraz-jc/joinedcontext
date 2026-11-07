/**
 * `PW_ONLY=gallery` (T-5575): ci-full's `ui-gallery` job starts the dev server alone and
 * runs the gallery project alone; without it both servers and both projects stay.
 */
import { afterEach, describe, expect, it, vi } from "vitest";

async function load(only?: string) {
  vi.resetModules();
  if (only === undefined) {
    vi.stubEnv("PW_ONLY", "");
  } else {
    vi.stubEnv("PW_ONLY", only);
  }
  const config = (await import("../playwright.config")).default;
  const servers = [config.webServer ?? []].flat().map((server) => server.command);
  const projects = (config.projects ?? []).map((project) => project.name);
  return { servers, projects };
}

describe("playwright.config PW_ONLY", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("starts both servers and both projects by default", async () => {
    const { servers, projects } = await load();
    expect(projects).toEqual(["chromium", "gallery"]);
    expect(servers).toHaveLength(2);
    expect(servers[0]).toContain("vite build");
  });

  it("starts only the gallery dev server and project under PW_ONLY=gallery", async () => {
    const { servers, projects } = await load("gallery");
    expect(projects).toEqual(["gallery"]);
    expect(servers).toEqual(["npx vite dev --port 4174 --strictPort --host 127.0.0.1"]);
  });

  it("ignores any other PW_ONLY value", async () => {
    const { projects } = await load("chromium");
    expect(projects).toEqual(["chromium", "gallery"]);
  });
});

describe("playwright.live.config", () => {
  // The live walker runs in containers whose /dev/shm is 64 MB: without this flag the tab
  // crashed on dev's large pages and the walk wrote no report (T-5992).
  it("keeps Chromium's shared memory off /dev/shm", async () => {
    const config = (await import("../playwright.live.config")).default;
    const args = (config.projects ?? []).map((project) => project.use?.launchOptions?.args);
    // One project per state group (e2e/live/stateGroups.ts): each must carry the flag.
    expect(args.length).toBeGreaterThan(0);
    for (const a of args) expect(a).toEqual(["--disable-dev-shm-usage"]);
  });
});
