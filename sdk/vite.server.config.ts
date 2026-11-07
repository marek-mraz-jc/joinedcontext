import { defineConfig } from "vite";

// The functions bundle: `src/sdk/functions-runtime.ts` in one ES module with no imports left, which
// the Portal sends to jc-functions as `@joinedcontext/sdk/server` with every invocation.
export default defineConfig({
  logLevel: "warn",
  build: {
    outDir: "dist",
    emptyOutDir: false,
    target: "es2022",
    lib: {
      entry: "src/sdk/functions-runtime.ts",
      formats: ["es"],
      fileName: () => "functions-server.js",
    },
  },
});
