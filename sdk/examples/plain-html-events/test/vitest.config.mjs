// The tests' configuration (T-3416), kept out of the served root: the page has no build step and
// no package manager (AP-83). Its tests run under vitest in a DOM, from the SDK's tooling
// (`npx vitest run --root examples/plain-html-events --config test/vitest.config.mjs` in sdk/) or
// in CI's throwaway copy, which puts this file at the copy's root. Paths are from the root.
export default {
  test: {
    environment: "jsdom",
    include: ["test/**/*.test.mjs"],
    setupFiles: ["./test/setup.mjs"],
    coverage: { include: ["app.js"] },
  },
};
