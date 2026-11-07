import { defineConfig, devices } from "@playwright/test";

const APP_PORT = 4318;
const STUB_PORT = 4319;
const BASE_PATH = "/apps/air-quality/";
const BASE_URL = `http://127.0.0.1:${APP_PORT}`;

// The flow runs against the real binary behind its real base path, with a stub in the
// endpoint's place. Playwright plays the edge: it sets the X-Access-Token and X-Userinfo headers
// the openid-connect plugin would set, which is the only way to see the difference between a
// steward and a viewer.
export default defineConfig({
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: { baseURL: BASE_URL, trace: "on-first-retry" },
  projects: [
    { name: "chromium", testDir: "../tests/e2e", use: { ...devices["Desktop Chrome"] } },
    // The four-width check sits beside the bundle, where `@joinedcontext/sdk` resolves (T-2825).
    { name: "widths", testDir: "./e2e", use: { ...devices["Desktop Chrome"] } },
  ],
  webServer: [
    {
      command: `node ../tests/e2e/stub-endpoint.mjs`,
      url: `http://127.0.0.1:${STUB_PORT}/health`,
      env: { STUB_PORT: String(STUB_PORT) },
      reuseExistingServer: !process.env.CI,
      timeout: 30_000,
    },
    {
      // The bundle first: a debug build of rust-embed reads ui/dist from disk, so the binary
      // serves whatever was last built without being recompiled.
      command: `npx vite build && cargo run -q --manifest-path ../Cargo.toml --bin air-quality`,
      url: `${BASE_URL}${BASE_PATH}`,
      env: {
        JC_BIND_ADDRESS: `127.0.0.1:${APP_PORT}`,
        JC_BASE_PATH: BASE_PATH,
        JC_ENDPOINT_URL: `http://127.0.0.1:${STUB_PORT}/`,
        JC_ME_URL: `http://127.0.0.1:${STUB_PORT}/me`,
      },
      reuseExistingServer: !process.env.CI,
      timeout: 300_000,
    },
  ],
});
