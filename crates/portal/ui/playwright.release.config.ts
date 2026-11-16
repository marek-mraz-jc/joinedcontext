import { defineConfig, devices } from "@playwright/test";

// Release e2e (T-7156): the journeys of e2e/release click through the whole Portal on throwaway full
// jc stacks that bins/jc/e2e/release/stacks.py starts (RELEASE_STACKS names its stacks.json); no
// stubbed API (e2e/release/lint.spec.ts refuses one). Workers = stacks x slots; worker p runs on
// stack p % stacks. `run.sh` in bins/jc/e2e/release starts the stacks, runs this, and tears down.
//   python3 bins/jc/e2e/release/stacks.py up --stacks 4 --slots 2 --run /tmp/rel
//   RELEASE_STACKS=/tmp/rel/stacks.json pnpm exec playwright test -c playwright.release.config.ts --workers 8
// RELEASE_RETRIES (default 1): a journey green only on its retry is a flake row in the report
// (report.json, `flaky`), never a pass.
const out = process.env.RELEASE_OUT ?? "test-results/release";
export default defineConfig({
  testDir: "./e2e/release",
  outputDir: `${out}/artifacts`,
  fullyParallel: true,
  retries: Number(process.env.RELEASE_RETRIES ?? "1"),
  reporter: [["list"], ["json", { outputFile: `${out}/report.json` }]],
  timeout: 180_000,
  expect: { timeout: 20_000 },
  use: {
    actionTimeout: 30_000,
    navigationTimeout: 60_000,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    viewport: { width: 1440, height: 900 },
  },
  projects: [
    { name: "lint", testMatch: /lint\.spec\.ts/ },
    {
      name: "journeys",
      testIgnore: /lint\.spec\.ts/,
      dependencies: ["lint"],
      use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 }, launchOptions: { args: ["--disable-dev-shm-usage"] } },
    },
  ],
});
