import { defineConfig } from "@playwright/test";

// The catalogue pages of a running dataroom (a jc build or the dataroom server), by hand:
//   CATALOGUE_BASE_URL=http://localhost:5000 CATALOGUE_DATASET=air-quality \
//     node <portal ui>/node_modules/@playwright/test/cli.js test -c crates/dataroom/web/e2e
export default defineConfig({
  testDir: ".",
  outputDir: "test-results/out",
  retries: 0,
  use: { baseURL: process.env.CATALOGUE_BASE_URL ?? "http://localhost:5000" },
});
