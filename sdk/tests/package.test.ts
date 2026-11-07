/**
 * T-0682: `@joinedcontext/sdk` is a package a generated application can install (SDK-17, AP-11).
 *
 * A published application is built in CI from its committed project, with the SDK installed from
 * the organization's forge registry rather than resolved to this checkout. So what the tarball
 * carries is the contract: every `exports` target has to be in it, the entry points have to be
 * built JavaScript with their declarations beside them, and the libraries the application pins
 * itself have to stay outside the bundle — one copy of React is the only copy that works.
 *
 * The build runs here rather than in a fixture because a stale `dist/package` would let the
 * package.json promise a file that the last build never wrote.
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

interface Manifest {
  name: string;
  version: string;
  private?: boolean;
  files: string[];
  exports: Record<string, string | Record<string, string>>;
  publishConfig?: { exports?: Record<string, string | Record<string, string>> };
  peerDependencies: Record<string, string>;
  peerDependenciesMeta?: Record<string, { optional?: boolean }>;
  dependencies: Record<string, string>;
}

const workspace = JSON.parse(readFileSync(join(root, "package.json"), "utf8")) as Manifest;

/**
 * The real tarball, built the way a publish builds it.
 *
 * Read from the tarball rather than from the checkout, because the two manifests are not the
 * same file: this repository's own `exports` point at the SDK's TypeScript sources, which is how
 * the Portal and the reference applications link it, and `publishConfig.exports` is what pnpm
 * writes into the published manifest instead. Only the second one is the contract a consumer
 * installs, so only the second one is worth asserting.
 */
function pack(): { dir: string; files: string[]; manifest: Manifest } {
  const dir = mkdtempSync(join(tmpdir(), "jc-sdk-pack-"));
  // `prepack` builds the package; a stale `dist/package` would let the manifest promise a file
  // the last build never wrote.
  rmSync(join(root, "dist/package"), { recursive: true, force: true });
  execFileSync("pnpm", ["pack", "--pack-destination", dir], { cwd: root, stdio: "ignore" });
  const [tarball] = readdirSync(dir).filter((name) => name.endsWith(".tgz"));
  const path = join(dir, tarball);
  const files = execFileSync("tar", ["-tzf", path], { encoding: "utf8" })
    .split("\n")
    .filter((name) => name !== "" && !name.endsWith("/"))
    .map((name) => name.replace(/^package\//, ""));
  const manifest = JSON.parse(
    execFileSync("tar", ["-xzOf", path, "package/package.json"], { encoding: "utf8" }),
  ) as Manifest;
  return { dir, files, manifest };
}

const targets = (entry: string | Record<string, string>): string[] =>
  typeof entry === "string" ? [entry] : Object.values(entry);

describe("the published package", () => {
  let files: string[];
  let manifest: Manifest;
  let dir: string;

  beforeAll(() => {
    ({ dir, files, manifest } = pack());
  }, 900_000);

  /** A peer only one entry needs, which an application installs when it uses that entry. */
  const optional = (name: string): boolean => manifest.peerDependenciesMeta?.[name]?.optional === true;

  afterAll(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  it("links its sources inside this repository and ships its build outside it", () => {
    // The Portal, the reference applications and the SDK's own journeys all build against
    // `src/sdk/*.ts` through `link:../sdk`, which installs nothing and runs no build. Pointing
    // the workspace `exports` at `dist/package` broke every one of those builds (T-2303).
    for (const target of Object.values(workspace.exports)) {
      expect(typeof target, "a workspace export is a single source file").toBe("string");
      expect(target).toMatch(/^\.\/src\/sdk\//);
    }
    // And pnpm replaces them with the built ones on the way into the tarball.
    expect(manifest.exports).toEqual(workspace.publishConfig?.exports);
    expect(manifest.publishConfig ?? null).toBeNull();
  });

  it("is publishable at the Portal's own version", () => {
    expect(manifest.name).toBe("@joinedcontext/sdk");
    expect(manifest.private).toBeUndefined();
    expect(manifest.version).toMatch(/^\d+\.\d+\.\d+$/);
    // The template a generated application starts from pins exactly this version (SDK-17).
    const template = JSON.parse(readFileSync(join(root, "template/package.json"), "utf8")) as {
      dependencies: Record<string, string>;
    };
    expect(template.dependencies["@joinedcontext/sdk"]).toBe(manifest.version);
  });

  it("carries every file its exports promise", () => {
    const promised = Object.values(manifest.exports).flatMap(targets);
    expect(promised.length).toBeGreaterThan(4);
    for (const target of promised) {
      const path = target.replace(/^\.\//, "");
      expect(existsSync(join(root, path)), `${target} was never built`).toBe(true);
      expect(files, `${target} is outside "files"`).toContain(path);
    }
  });

  it("ships src/sdk/style.css in its stylesheet, not only the components' CSS", () => {
    // `./style.css` is the build's one CSS asset, and the entries import only layout.css: the
    // typeface (T-3124) and MapLibre's attribution underline (T-3577) never reached an App built
    // on the forge, whose axe sweep kept failing link-in-text-block (T-3578).
    const css = readFileSync(join(root, "dist/package/style.css"), "utf8");
    expect(css).toMatch(/\.maplibregl-ctrl-attrib a\[href\]\{text-decoration:underline/);
    expect(css).toMatch(/body\{font-family:var\(--jc-font-body/);
    expect(css).toMatch(/@font-face\{font-family:"?Inter"?;/);
    expect(css).toContain("data:font/woff2");
    expect(css).toContain(".jc-shell{");
  });

  it("ships built JavaScript and declarations, never the TypeScript source", () => {
    for (const [name, entry] of Object.entries(manifest.exports)) {
      if (typeof entry === "string") continue;
      expect(entry.import, `${name} has no import target`).toMatch(/\.js$/);
      expect(entry.types, `${name} has no types`).toMatch(/\.d\.ts$/);
    }
    expect(files.some((file) => file.endsWith(".ts") && !file.endsWith(".d.ts"))).toBe(false);
    // The consumer's own `tsc -b` must not be handed this checkout's test setup.
    expect(files.some((file) => file.includes(".test."))).toBe(false);
  });

  it("leaves the application's own libraries outside the bundle", () => {
    // Every built file, entries and the chunks the root shares with `./grid` (T-6305): what they
    // still import by name was not copied into the bundle.
    const built = readdirSync(join(root, "dist/package"), { recursive: true, encoding: "utf8" }).filter((file) => file.endsWith(".js"));
    const imported = new Set(
      built
        .map((file) => readFileSync(join(root, "dist/package", file), "utf8"))
        .flatMap((code) => [...code.matchAll(/(?:from|import)\s*["']([^"']+)["']/g)].map(([, name]) => name)),
    );
    // Every peer is one the SDK's own code imports — `react-dom` as its `client` entry. An optional
    // peer serves one entry and is checked below.
    for (const peer of Object.keys(manifest.peerDependencies).filter((name) => !optional(name))) {
      expect(
        [...imported].some((name) => name === peer || name.startsWith(`${peer}/`)),
        `${peer} is declared a peer but nothing imports it`,
      ).toBe(true);
    }
    // An optional peer is found at run time by the entry that needs it (`axe-core` by
    // `./responsive`, through `import.meta.resolve`), so that entry has to name it.
    const entries = readdirSync(join(root, "dist/package"))
      .filter((file) => file.endsWith(".js"))
      .map((file) => readFileSync(join(root, "dist/package", file), "utf8"));
    for (const peer of Object.keys(manifest.peerDependencies).filter(optional)) {
      expect(
        entries.some((code) => code.includes(`"${peer}/`) || code.includes(`"${peer}"`)),
        `${peer} is declared an optional peer but no entry names it`,
      ).toBe(true);
    }
    expect(manifest.peerDependencies.react).toBeDefined();
    expect(manifest.dependencies.react).toBeUndefined();
    // What is left a dependency is bundled or installed for the consumer, never both.
    for (const own of Object.keys(manifest.dependencies)) {
      expect(manifest.peerDependencies[own]).toBeUndefined();
    }
    // `echartsTheme` and `rechartsPalette` return plain objects: the SDK never imports a chart
    // library, so a consumer must not be made to install one.
    for (const unused of ["echarts", "recharts", "@deck.gl/core"]) {
      expect(manifest.peerDependencies[unused]).toBeUndefined();
      expect(manifest.dependencies[unused]).toBeUndefined();
    }
  });

  it("is built for production, not for this checkout's development mode", () => {
    for (const entry of ["index.js", "grid.js", "server.js", "testing.js"]) {
      const built = readFileSync(join(root, "dist/package", entry), "utf8");
      expect(built, `${entry} ships React's development JSX runtime`).not.toContain(
        "jsx-dev-runtime",
      );
      expect(built, `${entry} left process.env in the bundle`).not.toContain("process.env.NODE_ENV");
    }
  });

  it("names the same peer versions the generated application pins", () => {
    const template = JSON.parse(readFileSync(join(root, "template/package.json"), "utf8")) as {
      dependencies: Record<string, string>;
      devDependencies?: Record<string, string>;
    };
    for (const [peer, range] of Object.entries(manifest.peerDependencies)) {
      if (optional(peer)) {
        // The build lane installs the template and nothing an application names (AP-82), and
        // runs every application's browser checks with it, so a peer one of those checks needs
        // (axe-core for the responsive check) is the template's to install (T-3184).
        const pinned = template.dependencies[peer] ?? template.devDependencies?.[peer];
        expect(pinned, `${peer} is not installed by the template the build lane runs`).toBe(range);
        continue;
      }
      // A peer the template does not pin would be resolved twice, which is how two Reacts happen.
      expect(template.dependencies[peer], `${peer} is not pinned by the template`).toBe(range);
    }
  });

  it("carries no secret and no registry credential", () => {
    for (const file of files) {
      expect(file).not.toMatch(/\.(env|pem|key)$/);
      expect(file).not.toMatch(/npmrc/);
    }
  });
});
