import { ProblemError } from "@joinedcontext/sdk";
import type { PanelSource, Row, Schema } from "@joinedcontext/sdk";
import { ApiError, getStations, updateStation } from "./api";
import type { Identity, Station } from "./api";

/** The type every station of this App is (the `helsinki` model). */
export const STATION_TYPE = "AirQualityObserved";

/**
 * What the entity panel shows of a station and how it names each part (SDK-40). The note is the
 * one attribute a steward corrects in the panel: one to 500 characters, as the backend checks, so
 * it is required; the name per language and the position stay with the record form.
 */
export const SCHEMA = {
  [STATION_TYPE]: {
    properties: {
      name: { type: "string", title: "Name" },
      pm10: { type: "number", title: "PM10 (µg/m³)" },
      pm25: { type: "number", title: "PM2.5 (µg/m³)" },
      airQualityIndex: { type: "number", title: "Air quality index" },
      dateObserved: { type: "string", title: "Measured" },
      stewardNote: { type: "string", title: "Steward note" },
    },
    required: ["stewardNote"],
  },
} satisfies Schema;

/** A station as the panel lists it: only what the backend answered, nothing invented. */
export function rowOf(station: Station): Row {
  const row: Row = { id: station.id, type: STATION_TYPE };
  if (station.name !== undefined) row.name = station.name;
  if (station.pm10 !== undefined) row.pm10 = station.pm10;
  if (station.pm25 !== undefined) row.pm25 = station.pm25;
  if (station.airQualityIndex !== undefined) row.airQualityIndex = station.airQualityIndex;
  if (station.observedAt !== undefined) row.dateObserved = station.observedAt;
  if (station.stewardNote !== undefined) row.stewardNote = station.stewardNote;
  return row;
}

/**
 * The station's page in the Portal, for a reader who may not change it here. An App is served
 * on `{name}.apps.{domain}` (AP-133) and the Portal on `portal.{domain}`; on any other host (a
 * preview, a test) there is no link rather than a guessed one.
 */
export function portalLinkOf(hostname: string, id: string): string | null {
  const match = /^[a-z0-9-]+\.apps\.([a-z0-9.-]+)$/.exec(hostname);
  if (!match) return null;
  return `https://portal.${match[1]}/projects/helsinki/explore?space=helsinki&entityId=${encodeURIComponent(id)}`;
}

function asProblem(cause: unknown): ProblemError {
  if (cause instanceof ApiError) return new ProblemError(cause.status, { title: cause.detail });
  return new ProblemError(0, { title: cause instanceof Error ? cause.message : String(cause) });
}

/**
 * The panel's source: this App's own backend, which holds the endpoint (AP-04). A steward may
 * change the note; everyone else reads, and is linked to the Portal. `onChanged` reloads the page.
 */
export function stationSource(identity: Identity | null, onChanged: () => void, hostname = window.location.hostname): PanelSource {
  const steward = identity?.roles.includes("steward") ?? false;
  return {
    async get(entity) {
      let stations: Station[];
      try {
        stations = await getStations();
      } catch (cause) {
        throw asProblem(cause);
      }
      const found = stations.find((station) => station.id === entity.id);
      if (!found) throw new ProblemError(404, { title: "This station is no longer there." });
      return rowOf(found);
    },
    async update(entity, patch) {
      const note = patch.stewardNote;
      if (typeof note !== "string" || Object.keys(patch).some((name) => name !== "stewardNote")) {
        throw new ProblemError(400, { title: "Only the steward note is changed here." });
      }
      try {
        await updateStation(entity.id, { stewardNote: note });
      } catch (cause) {
        throw asProblem(cause);
      }
      onChanged();
    },
    mayEdit: (type, attr) => steward && type === STATION_TYPE && (attr === undefined || attr === "stewardNote"),
    portalLink: (entity) => (steward ? null : portalLinkOf(hostname, entity.id)),
    schema: SCHEMA,
    language: "en",
  };
}
