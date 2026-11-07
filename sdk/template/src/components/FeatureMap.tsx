import { useEffect, useState } from "react";
import { ProblemError, useClient } from "@joinedcontext/sdk";
import type { Cell, FeatureCollection, Row } from "@joinedcontext/sdk";
import { EntityMap } from "./EntityMap";
import { Problem } from "./states";

/** The rows a page of Features stands for: its URN, the type, its properties and its geometry under `location`. */
export function rowsOf(type: string, page: FeatureCollection, location: string): Row[] {
  return page.features.map((f) => ({ ...(f.properties as Record<string, Cell>), id: f.id, type, [location]: f.geometry }));
}

type MapProps = Omit<Parameters<typeof EntityMap>[0], "rows" | "location">;

/**
 * A type's map read from the endpoint's OGC API - Features surface (EP-29): one `/items` page, the
 * same Entities the endpoint's view and Policy admit, drawn as GeoJSON. An endpoint that does not
 * serve OGC Features answers 404, and the map draws `fallback`, the rows the page already read, so
 * an App on such an endpoint keeps its map. Until the Features arrive the fallback is drawn too.
 */
export function FeatureMap({ type, endpoint, fallback, location = "location", ...map }: { type: string; endpoint?: string; fallback: Row[]; location?: string } & MapProps) {
  const client = useClient();
  const [read, setRead] = useState<Row[] | null>(null);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let gone = false;
    setRead(null);
    setError(null);
    client.features.items(type, { endpoint, limit: 1000 }).then(
      (page) => {
        if (!gone) setRead(rowsOf(type, page, location));
      },
      (failure: unknown) => {
        if (gone || (failure instanceof ProblemError && failure.status === 404)) return;
        setError(failure instanceof Error ? failure : new Error(String(failure)));
      },
    );
    return () => {
      gone = true;
    };
  }, [client, type, endpoint, location]);

  return (
    <>
      <Problem error={error} />
      <EntityMap {...map} rows={read ?? fallback} location={location} />
    </>
  );
}
