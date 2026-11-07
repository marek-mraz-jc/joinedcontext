/**
 * The entity grid's example page (`vite dev`, then `/grid.html`): the same component twice, from
 * two JSON configs — an Endpoint in view mode and a Context Space in edit mode — with no code
 * difference between them (T-1428, T-1438). A stub transport answers both path shapes from a
 * fixture, so the page runs without a platform.
 */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { Transport } from "../sdk/transport";
import "../sdk/style.css";
import { EntityGrid } from "./EntityGrid";
import { parseGridConfig } from "./config";
import { sourceFor } from "./source";

const station = (n: number, name: string, bikes: number, at: string) => ({
  id: `urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki-bikes:${n.toString().padStart(3, "0")}`,
  type: "BikeHireDockingStation",
  createdAt: "2026-09-01T06:00:00Z",
  modifiedAt: at,
  name: { type: "Property", value: name },
  availableBikeNumber: { type: "Property", value: bikes, unitCode: "C62", observedAt: at },
  totalSlotNumber: { type: "Property", value: 20, unitCode: "C62" },
  refDevice: { type: "Relationship", object: `urn:ngsi-ld:Device:hel.fi:helsinki-bikes:dock-${n}` },
  location: { type: "GeoProperty", value: { type: "Point", coordinates: [24.93 + n / 1000, 60.17] } },
});

const entities = [
  station(1, "Kamppi", 7, "2026-09-18T08:00:00Z"),
  station(2, "Kaisaniemi", 0, "2026-09-18T08:01:00Z"),
  station(3, "Hakaniemi", 12, "2026-09-18T08:02:00Z"),
];

const transport: Transport = async ({ method, path }) =>
  method === "GET" && path.includes("/ngsi-ld/v1/entities?") ? { status: 200, body: entities } : { status: 404, body: null };

const configs: unknown[] = [
  {
    source: { kind: "endpoint", slug: "bikes0example0slug000000000" },
    type: "BikeHireDockingStation",
    columns: [{ attr: "name" }, { attr: "availableBikeNumber", show: { observedAt: true } }, { attr: "refDevice" }],
    pageSize: 25,
  },
  {
    source: { kind: "space", space: "helsinki-bikes" },
    type: "BikeHireDockingStation",
    entityTimestamps: true,
    mode: "edit",
    editableAttrs: ["name"],
    density: "compact",
  },
];

function Grids() {
  return (
    <main style={{ padding: 16, display: "grid", gap: 24 }}>
      {configs.map((raw, index) => {
        const { config, findings } = parseGridConfig(raw);
        if (!config) return <pre key={index}>{JSON.stringify(findings, null, 2)}</pre>;
        if (config.source.kind === "fixture") return null;
        return <EntityGrid key={index} config={config} source={sourceFor(config.source, transport)} />;
      })}
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Grids />
  </StrictMode>,
);
