/**
 * "Use this data" (T-3254): the explorer's view as code a person runs as written, in curl, Python
 * and JavaScript, against the endpoint they read through and with the filter they set. A public
 * endpoint needs nothing; any other reads the token from the `JC_TOKEN` environment variable, so
 * no secret is ever part of a snippet, a copy or a screenshot.
 */
import type { EntityQuery } from "../../components/entities/filters";

export type SnippetLanguage = "curl" | "python" | "javascript";
export const SNIPPET_LANGUAGES: SnippetLanguage[] = ["curl", "python", "javascript"];

/** The environment variable every snippet reads its token from. */
export const TOKEN_VARIABLE = "JC_TOKEN";
/** How many entities a snippet asks for: one page, the most a call answers. */
export const SNIPPET_LIMIT = 100;

/** The view's entities as one NGSI-LD call to the endpoint, every value URL-encoded. */
export function viewPath(query: EntityQuery): string {
  const params = new URLSearchParams();
  if (query.type) params.set("type", query.type);
  if (query.q?.trim()) params.set("q", query.q.trim());
  if (query.scopeQ?.trim()) params.set("scopeQ", query.scopeQ.trim());
  if (query.idPattern?.trim()) params.set("idPattern", query.idPattern.trim());
  if (query.attrs && query.attrs.length > 0) params.set("attrs", query.attrs.join(","));
  params.set("options", "keyValues");
  params.set("limit", String(SNIPPET_LIMIT));
  return `/ngsi-ld/v1/entities?${params.toString()}`;
}

/**
 * The snippet of one language. `url` is the whole address; it is quoted for the shell with single
 * quotes (an encoded URL holds none) and written as a JSON string literal for Python and
 * JavaScript, which both read it as is.
 */
export function snippet(language: SnippetLanguage, url: string, open: boolean): string {
  const literal = JSON.stringify(url);
  switch (language) {
    case "curl":
      return [
        `curl -sS --fail-with-body \\`,
        `  -H 'Accept: application/json' \\`,
        ...(open ? [] : [`  -H "Authorization: Bearer $${TOKEN_VARIABLE}" \\`]),
        `  '${url.replace(/'/g, "%27")}'`,
      ].join("\n");
    case "python":
      return [
        "import json, os, urllib.request",
        "",
        `url = ${literal}`,
        'headers = {"Accept": "application/json"}',
        ...(open ? [] : [`headers["Authorization"] = "Bearer " + os.environ["${TOKEN_VARIABLE}"]`]),
        "with urllib.request.urlopen(urllib.request.Request(url, headers=headers)) as response:",
        "    rows = json.load(response)",
        'print(len(rows), "rows")',
        "for row in rows[:5]:",
        '    print(row["id"])',
      ].join("\n");
    case "javascript":
      return [
        `// Node 18 or later: node use-this-data.mjs`,
        `const url = ${literal};`,
        `const headers = { Accept: "application/json" };`,
        ...(open ? [] : [`headers.Authorization = \`Bearer \${process.env.${TOKEN_VARIABLE}}\`;`]),
        `const response = await fetch(url, { headers });`,
        `if (!response.ok) throw new Error(\`\${response.status} \${await response.text()}\`);`,
        `const rows = await response.json();`,
        `console.log(rows.length, "rows");`,
        `for (const row of rows.slice(0, 5)) console.log(row.id);`,
      ].join("\n");
  }
}
