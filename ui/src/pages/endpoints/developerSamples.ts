/**
 * The samples of a public Endpoint's developer page (T-5111): one tab per surface the Endpoint
 * serves, each a call a developer copies and runs as it stands, anonymously. Every address comes
 * from the Portal's standards answer, which keeps a first page only when it answered with
 * entities; nothing here names a token, since a public page never asks for one.
 */
export type Language = "curl" | "python" | "javascript";
/** A program a desktop tool is, with the steps that connect it. */
export type Tool = "qgis" | "arcgis" | "excel" | "calendar" | "claude";

export interface SurfaceSamples {
  surface: "ngsi-ld" | "ogc-features" | "geojson" | "odata" | "sta" | "csv" | "ical" | "websocket" | "mcp";
  /** The address the samples call. */
  url: string;
  /** The call in each language that can make it: no shell tool opens a WebSocket. */
  code: Partial<Record<Language, string>>;
  /** Desktop tools that open the surface, each with the address it is given. */
  tools: { tool: Tool; url: string }[];
}

/** What the samples are built from: the Portal's standards answer of the Endpoint. */
export interface SampleInput {
  base: string;
  served: string[];
  entityType?: string | null;
  standards: { representation: string; first?: string | null }[];
}

/** A value a shell reads as one word, whatever it holds. */
function quoted(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

/** A read over HTTP in each language; `file` saves the answer instead of printing it. */
function read(url: string, accept: string, file?: string): Record<Language, string> {
  const json = accept.includes("json");
  return {
    curl: `curl -sSf -H ${quoted(`Accept: ${accept}`)} ${file ? `-o ${file} ` : ""}${quoted(url)}`,
    python: [
      json ? "import json" : null,
      "import urllib.request",
      "",
      `request = urllib.request.Request(${JSON.stringify(url)}, headers={"Accept": ${JSON.stringify(accept)}})`,
      "with urllib.request.urlopen(request, timeout=30) as answer:",
      json ? "    print(json.dumps(json.load(answer), indent=2, ensure_ascii=False))" : '    print(answer.read().decode("utf-8")[:2000])',
    ]
      .filter((line) => line !== null)
      .join("\n"),
    javascript: [
      `const answer = await fetch(${JSON.stringify(url)}, { headers: { Accept: ${JSON.stringify(accept)} } });`,
      "if (!answer.ok) throw new Error(`HTTP ${answer.status}`);",
      json ? "console.log(await answer.json());" : "console.log((await answer.text()).slice(0, 2000));",
    ].join("\n"),
  };
}

/** The subscription a WebSocket sample sends: the example type, or every entity with a location. */
export function subscribeMessage(entityType?: string | null): string {
  const body: Record<string, unknown> = { type: "Subscription" };
  if (entityType) body.entities = [{ type: entityType }];
  else body.watchedAttributes = ["location"];
  return JSON.stringify({ metadata: { "x-op": "x-subscribe", "x-id": "s1" }, body });
}

const MCP_INITIALIZE = JSON.stringify({
  jsonrpc: "2.0",
  id: 1,
  method: "initialize",
  params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "developer-page", version: "1" } },
});

export function samplesOf(input: SampleInput): SurfaceSamples[] {
  const base = input.base.endsWith("/") ? input.base : `${input.base}/`;
  const first = (representation: string) => input.standards.find((s) => s.representation === representation)?.first ?? undefined;
  const serves = (surface: string) => input.served.includes(surface);
  const out: SurfaceSamples[] = [];
  if (serves("ngsi-ld")) {
    const url = first("ngsi-ld") ?? `${base}ngsi-ld/v1/types`;
    out.push({ surface: "ngsi-ld", url, code: read(url, "application/ld+json"), tools: [] });
  }
  if (serves("ogc-features")) {
    const items = first("ogc-features");
    const url = items ?? `${base}ogc/features/collections`;
    const tools: SurfaceSamples["tools"] = [
      { tool: "qgis", url: `${base}ogc/features` },
      { tool: "arcgis", url: `${base}ogc/features` },
    ];
    out.push({ surface: "ogc-features", url, code: read(url, "application/json"), tools });
    if (items) out.push({ surface: "geojson", url: items, code: read(items, "application/geo+json", "data.geojson"), tools: [] });
  }
  if (serves("odata")) {
    const url = first("odata") ?? `${base}odata/v4/`;
    out.push({ surface: "odata", url, code: read(url, "application/json"), tools: [{ tool: "excel", url: `${base}odata/v4/` }] });
  }
  if (serves("sta")) {
    const url = first("sta") ?? `${base}sta/v1.1`;
    out.push({ surface: "sta", url, code: read(url, "application/json"), tools: [] });
  }
  if (serves("csv")) {
    const url = `${base}file.csv`;
    out.push({ surface: "csv", url, code: read(url, "text/csv", "data.csv"), tools: [] });
  }
  if (serves("ical")) {
    const url = `${base}ical/v1/events.ics`;
    out.push({ surface: "ical", url, code: read(url, "text/calendar", "events.ics"), tools: [{ tool: "calendar", url: url.replace(/^https?:\/\//, "webcal://") }] });
  }
  if (serves("websocket")) {
    const url = `${base.replace(/^http/, "ws")}ws`;
    const message = subscribeMessage(input.entityType);
    out.push({
      surface: "websocket",
      url,
      code: {
        python: [
          "import asyncio",
          "import websockets",
          "",
          "async def main():",
          `    async with websockets.connect(${JSON.stringify(url)}, subprotocols=["ngsi-ld-ws"]) as socket:`,
          `        await socket.send(${JSON.stringify(message)})`,
          "        print(await asyncio.wait_for(socket.recv(), 30))",
          "",
          "asyncio.run(main())",
        ].join("\n"),
        javascript: [
          `const socket = new WebSocket(${JSON.stringify(url)}, "ngsi-ld-ws");`,
          `socket.onopen = () => socket.send(${JSON.stringify(message)});`,
          "socket.onmessage = (event) => {",
          "  console.log(event.data);",
          "  socket.close();",
          "};",
          'socket.onerror = () => console.error("The WebSocket did not open");',
        ].join("\n"),
      },
      tools: [],
    });
  }
  if (serves("mcp")) {
    const url = `${base}mcp`;
    const headers = { "Content-Type": "application/json", Accept: "application/json, text/event-stream" };
    out.push({
      surface: "mcp",
      url,
      code: {
        curl: `curl -sSf -X POST -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data ${quoted(MCP_INITIALIZE)} ${quoted(url)}`,
        python: [
          "import urllib.request",
          "",
          `request = urllib.request.Request(${JSON.stringify(url)}, method="POST", data=${JSON.stringify(MCP_INITIALIZE)}.encode(), headers=${JSON.stringify(headers)})`,
          "with urllib.request.urlopen(request, timeout=30) as answer:",
          '    print(answer.read().decode("utf-8"))',
        ].join("\n"),
        javascript: [
          `const answer = await fetch(${JSON.stringify(url)}, {`,
          '  method: "POST",',
          `  headers: ${JSON.stringify(headers)},`,
          `  body: ${JSON.stringify(MCP_INITIALIZE)},`,
          "});",
          "if (!answer.ok) throw new Error(`HTTP ${answer.status}`);",
          "console.log(await answer.text());",
        ].join("\n"),
      },
      tools: [{ tool: "claude", url }],
    });
  }
  return out;
}
