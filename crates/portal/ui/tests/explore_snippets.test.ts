// @vitest-environment node
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/explore/snippets.ts.
// "Use this data" (T-3254): every snippet runs as written. Each one is executed here, by the
// shell's curl, python3 and node, against a local server standing in for the endpoint, which
// answers rows only to the call the view describes and, for a closed endpoint, only with the token
// taken from JC_TOKEN. No token is ever part of a snippet.
import { execFile } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { SNIPPET_LANGUAGES, snippet, viewPath } from "../src/pages/explore/snippets";

const run = promisify(execFile);
const TOKEN = "jc_k1_s3cr3t";
const ROWS = [
  { id: "urn:ngsi-ld:BikeStation:1", type: "BikeStation", bikes: 0 },
  { id: "urn:ngsi-ld:BikeStation:2", type: "BikeStation", bikes: 3 },
];
const QUERY = { type: "BikeStation", q: 'bikes<5;name=="Kamppi \'x\'"', attrs: ["bikes", "name"], idPattern: "^urn:" };

let base = "";
let dir = "";
const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", "http://localhost");
  const closed = url.pathname.startsWith("/closed/");
  const asked = url.pathname.replace(/^\/(open|closed)/, "") + url.search;
  const authorized = !closed || request.headers.authorization === `Bearer ${TOKEN}`;
  if (!authorized) {
    response.writeHead(401, { "Content-Type": "application/json" });
    response.end(JSON.stringify({ title: "Unauthorized" }));
    return;
  }
  if (asked !== viewPath(QUERY) || request.headers.accept !== "application/json") {
    response.writeHead(400, { "Content-Type": "application/json" });
    response.end(JSON.stringify({ title: "Bad request", asked, accept: request.headers.accept }));
    return;
  }
  response.writeHead(200, { "Content-Type": "application/json" });
  response.end(JSON.stringify(ROWS));
});

beforeAll(async () => {
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  dir = await mkdtemp(join(tmpdir(), "snippets-"));
});
afterAll(async () => {
  server.close();
  await rm(dir, { recursive: true, force: true });
});

async function execute(language: (typeof SNIPPET_LANGUAGES)[number], code: string, env: Record<string, string>): Promise<string> {
  const options = { env: { ...process.env, ...env }, timeout: 20_000 };
  if (language === "curl") return (await run("sh", ["-c", code], options)).stdout;
  const file = join(dir, language === "python" ? "snippet.py" : "snippet.mjs");
  await writeFile(file, code);
  return (await run(language === "python" ? "python3" : process.execPath, [file], options)).stdout;
}

describe("the snippets of a view", () => {
  it("encode the filter, the columns and the id pattern, and ask for plain rows", () => {
    const path = viewPath(QUERY);
    const params = new URL(`http://x${path}`).searchParams;
    expect(params.get("q")).toBe(QUERY.q);
    expect(params.get("attrs")).toBe("bikes,name");
    expect(params.get("idPattern")).toBe("^urn:");
    expect(params.get("options")).toBe("keyValues");
    expect(path).not.toMatch(/['" ]/);
  });

  it("never carry a token, and name the variable only for a closed endpoint", () => {
    for (const language of SNIPPET_LANGUAGES) {
      expect(snippet(language, "https://x/y", true)).not.toContain("JC_TOKEN");
      expect(snippet(language, "https://x/y", false)).toContain("JC_TOKEN");
      expect(snippet(language, "https://x/y", false)).not.toContain(TOKEN);
    }
  });

  it.each(SNIPPET_LANGUAGES)("%s runs as written against an open endpoint and prints the rows", async (language) => {
    const out = await execute(language, snippet(language, `${base}/open${viewPath(QUERY)}`, true), {});
    // curl hands the rows over as they came; the programs count them and name the first ones.
    if (language === "curl") expect(JSON.parse(out)).toEqual(ROWS);
    else expect(out).toContain("2 rows");
    expect(out).toContain("urn:ngsi-ld:BikeStation:1");
  });

  it.each(SNIPPET_LANGUAGES)("%s sends the token from JC_TOKEN to a closed endpoint", async (language) => {
    const out = await execute(language, snippet(language, `${base}/closed${viewPath(QUERY)}`, false), { JC_TOKEN: TOKEN });
    expect(out).toContain("urn:ngsi-ld:BikeStation:2");
  });

  it.each(SNIPPET_LANGUAGES)("%s fails loudly without the token instead of printing nothing", async (language) => {
    await expect(execute(language, snippet(language, `${base}/closed${viewPath(QUERY)}`, false), { JC_TOKEN: "" })).rejects.toThrow();
  });
});
