/**
 * "Use this data" (EP-82): how to read a dataset's Endpoint from a shell, from an App built on
 * the platform, and from an MCP client. Every snippet is built from the Endpoint URL the Portal
 * derived on its own host, and offers only what the Endpoint serves.
 */
export interface Snippet {
  key: "curl" | "csv" | "sdk" | "mcp";
  language: "shell" | "ts" | "json";
  code: string;
}

/** A value a shell reads as one word, whatever it holds. */
function quoted(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

export function snippets(
  endpointUrl: string,
  representations: string[],
  entityType: string | undefined,
  datasetName: string,
): Snippet[] {
  const base = endpointUrl.endsWith("/") ? endpointUrl : `${endpointUrl}/`;
  const out: Snippet[] = [];
  if (representations.includes("ngsi-ld") && entityType) {
    const query = new URLSearchParams({ type: entityType, limit: "10", options: "keyValues" });
    out.push({
      key: "curl",
      language: "shell",
      code: `curl -H 'Accept: application/json' ${quoted(`${base}ngsi-ld/v1/entities?${query.toString()}`)}`,
    });
    out.push({
      key: "sdk",
      language: "ts",
      code: [
        'import { useEntities } from "@joinedcontext/sdk";',
        "",
        `const { rows, loading, error } = useEntities(${JSON.stringify(entityType)});`,
      ].join("\n"),
    });
  }
  if (representations.includes("csv")) {
    out.push({ key: "csv", language: "shell", code: `curl -o data.csv ${quoted(`${base}file.csv`)}` });
  }
  if (representations.includes("mcp")) {
    const server = datasetName.replace(/[^a-z0-9-]/gi, "-").toLowerCase() || "dataset";
    out.push({
      key: "mcp",
      language: "json",
      code: JSON.stringify({ mcpServers: { [server]: { type: "http", url: `${base}mcp` } } }, null, 2),
    });
  }
  return out;
}
