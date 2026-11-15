// The renderer journeys: Chromium, every host routed inside the test. WP_URL (wordpress/run.sh)
// runs the WordPress page of T-5129 instead.
export default {
  testDir: ".",
  testMatch: process.env.WP_URL ? /wordpress\.spec\.mjs$/ : /(journey|explore|catalogue-map|embed|widget)\.spec\.mjs$/,
  timeout: 60000,
  workers: 2,
  use: { browserName: "chromium" }
};
