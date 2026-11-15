import { useCallback, useEffect, useId, useMemo, useState } from "react";
import type { JSX } from "react";
import { AppShell, Card, Grid, Page, Split, useEntitySelection } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { ApiError, createStation, deleteStation, getHistory, getIdentity, getStations, updateStation } from "./api";
import { Chart } from "./Chart";
import { historyOf } from "./quality";
import type { History } from "./quality";
import { STATION_TYPE, stationSource } from "./panel";
import { StationMap } from "./StationMap";
import type { Identity, Station, StationFields } from "./api";

/** Why a signed-in person without the role sees the controls disabled (UI-44). */
const READ_ONLY = "Only a steward adds, corrects or removes station records.";
/** Why the measured values have no field in the form. */
const MEASURED = "PM10, PM2.5 and the index are measured by the station and cannot be edited.";

/**
 * The whole app in the SDK's shell (SDK-39): who you are, what the stations read, and the record
 * form for a steward. A station on the map or its card opens the SDK's entity panel (SDK-40),
 * which reads and writes through this App's backend. The form follows the roles the Portal
 * answered (AP-109); what it hides here the gateway also refuses, and a refusal is shown in the
 * gateway's own words (AP-40).
 */
export function App(): JSX.Element {
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [stations, setStations] = useState<Station[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [me, list] = await Promise.all([getIdentity(), getStations()]);
      setIdentity(me);
      setStations(list);
      setError(null);
    } catch (cause) {
      setError(problem(cause));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const source = useMemo(() => stationSource(identity, () => void load()), [identity, load]);
  const pages = useMemo(
    (): ShellPage[] => [
      { id: "stations", label: "Stations", render: () => <StationsPage identity={identity} stations={stations} error={error} load={load} /> },
    ],
    [identity, stations, error, load],
  );
  const name = identity?.signedIn ? (identity.email ?? identity.user ?? undefined) : undefined;
  return <AppShell title="Air quality" userName={name} source={source} pages={pages} />;
}

function StationsPage({
  identity,
  stations,
  error,
  load,
}: {
  identity: Identity | null;
  stations: Station[] | null;
  error: string | null;
  load: () => Promise<void>;
}): JSX.Element {
  const { select } = useEntitySelection();
  // The station the map and the chart are about: the one a person picked, else the first.
  const [picked, setPicked] = useState<string | null>(null);
  const selected = stations?.find((station) => station.id === picked) ?? stations?.[0] ?? null;

  const steward = identity?.roles.includes("steward") ?? false;
  const readOnly = (identity?.signedIn ?? false) && !steward;

  const list = (
    <>
      {stations && stations.length === 0 && <p>No stations here yet.</p>}
      {stations && stations.length > 0 && (
        <Grid columns={steward ? 2 : 4}>
          {stations.map((station) => (
            <StationCard
              key={station.id}
              station={station}
              steward={steward}
              readOnly={readOnly}
              onSaved={load}
              onOpen={() => select({ id: station.id, type: STATION_TYPE })}
            />
          ))}
        </Grid>
      )}
    </>
  );

  return (
    <Page>
      {/* The shell names a signed-in reader; the page says only what the shell cannot. */}
      {identity && !identity.signedIn && <p className="identity">You are viewing anonymously.</p>}
      {readOnly && <p id="read-only-reason">{READ_ONLY}</p>}

      {error && <p role="alert">{error}</p>}
      {!stations && !error && <p role="status">Loading stations…</p>}

      {stations && stations.length > 0 && (
        <Split ratio="1:1">
          <Card title="Stations by air quality index">
            <StationMap
              stations={stations}
              selected={selected?.id ?? null}
              onSelect={(id) => {
                setPicked(id);
                select({ id, type: STATION_TYPE });
              }}
            />
          </Card>
          <Card title={`Last 24 hours at ${selected?.name ?? selected?.id ?? ""}`}>
            {selected && <StationHistory id={selected.id} name={selected.name ?? selected.id} />}
          </Card>
        </Split>
      )}

      {steward ? (
        <Split ratio="1:2">
          <Card title="Add a station">
            <StationForm onSaved={load} />
          </Card>
          {list}
        </Split>
      ) : (
        list
      )}
    </Page>
  );
}

/** One day of the picked station, read again when another station is picked. */
function StationHistory({ id, name }: { id: string; name: string }): JSX.Element {
  const [history, setHistory] = useState<{ id: string; data: History } | null>(null);
  const [error, setError] = useState<{ id: string; text: string } | null>(null);
  useEffect(() => {
    let current = true;
    getHistory(id)
      .then((entity) => current && setHistory({ id, data: historyOf(entity) }))
      .catch((cause: unknown) => current && setError({ id, text: problem(cause) }));
    return () => {
      current = false;
    };
  }, [id]);
  if (error?.id === id) return <p role="alert">{error.text}</p>;
  if (history?.id !== id) return <p role="status">Loading the last 24 hours…</p>;
  return <Chart history={history.data} station={name} />;
}

function problem(cause: unknown): string {
  return cause instanceof ApiError ? cause.detail : String(cause);
}

function StationCard({
  station,
  steward,
  readOnly,
  onSaved,
  onOpen,
}: {
  station: Station;
  steward: boolean;
  readOnly: boolean;
  onSaved: () => Promise<void>;
  onOpen: () => void;
}): JSX.Element {
  const [editing, setEditing] = useState(false);
  const title = station.name ?? station.id;
  return (
    <Card
      title={title}
      actions={
        <button type="button" onClick={onOpen}>
          Details of {title}
        </button>
      }
    >
      <dl>
        <Metric label="PM10" value={station.pm10} unit="µg/m³" />
        <Metric label="PM2.5" value={station.pm25} unit="µg/m³" />
        <Metric label="Index" value={station.airQualityIndex} />
      </dl>
      {station.observedAt && <p className="identity">Measured: {station.observedAt}</p>}
      {station.stewardNote && <p>Note: {station.stewardNote}</p>}
      {readOnly && (
        <button type="button" disabled aria-describedby="read-only-reason">
          Edit {title}
        </button>
      )}
      {steward && !editing && (
        <div className="actions">
          <button type="button" onClick={() => setEditing(true)}>
            Edit {title}
          </button>
          <RemoveButton station={station} title={title} onSaved={onSaved} />
        </div>
      )}
      {steward && editing && (
        <StationForm
          station={station}
          onSaved={async () => {
            setEditing(false);
            await onSaved();
          }}
          onCancel={() => setEditing(false)}
        />
      )}
    </Card>
  );
}

/** Removes a station a steward added, after a second click; a pipeline's station says why not. */
function RemoveButton({
  station,
  title,
  onSaved,
}: {
  station: Station;
  title: string;
  onSaved: () => Promise<void>;
}): JSX.Element {
  const [asked, setAsked] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const reason = useId();
  if (!station.own) {
    return (
      <>
        <button type="button" disabled aria-describedby={reason}>
          Remove {title}
        </button>
        <span id={reason} className="identity">
          {" "}Only a station a steward added can be removed.
        </span>
      </>
    );
  }
  return (
    <>
      <button
        type="button"
        onClick={() => {
          if (!asked) {
            setAsked(true);
            return;
          }
          deleteStation(station.id)
            .then(onSaved)
            .catch((cause: unknown) => {
              setAsked(false);
              setError(problem(cause));
            });
        }}
      >
        {asked ? `Confirm removal of ${title}` : `Remove ${title}`}
      </button>
      {error && <p role="alert">{error}</p>}
    </>
  );
}

function Metric({
  label,
  value,
  unit,
}: {
  label: string;
  value: number | undefined;
  unit?: string;
}): JSX.Element | null {
  // An attribute the grant hides never arrives, and an empty row would suggest a zero.
  if (value === undefined) {
    return null;
  }
  return (
    <>
      <dt>{label}</dt>
      <dd>
        {value}
        {unit ? ` ${unit}` : ""}
      </dd>
    </>
  );
}

/**
 * The record form: a new station when `station` is absent, a correction of it otherwise. A save
 * is one request to the backend, which makes one write through the endpoint (AP-62).
 */
function StationForm({
  station,
  onSaved,
  onCancel,
}: {
  station?: Station;
  onSaved: () => Promise<void>;
  onCancel?: () => void;
}): JSX.Element {
  const id = useId();
  const [localId, setLocalId] = useState("");
  const [fi, setFi] = useState(station?.names?.fi ?? (station?.names ? "" : (station?.name ?? "")));
  const [en, setEn] = useState(station?.names?.en ?? "");
  const [longitude, setLongitude] = useState(station?.coordinates ? String(station.coordinates[0]) : "");
  const [latitude, setLatitude] = useState(station?.coordinates ? String(station.coordinates[1]) : "");
  const [note, setNote] = useState(station?.stewardNote ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const complete = (fi.trim() !== "" || en.trim() !== "") && longitude !== "" && latitude !== "" && (station || localId.trim() !== "");
  const field = (name: string) => `${id}-${name}`;

  return (
    <form
      className="note"
      aria-label={station ? `Edit ${station.name ?? station.id}` : "New station"}
      onSubmit={(event) => {
        event.preventDefault();
        const fields: StationFields = {
          name: { ...station?.names, fi: fi.trim(), en: en.trim() },
          coordinates: [Number(longitude), Number(latitude)],
          ...(note.trim() ? { stewardNote: note.trim() } : {}),
          ...(station ? {} : { localId: localId.trim() }),
        };
        setSaving(true);
        setError(null);
        (station ? updateStation(station.id, fields) : createStation(fields))
          .then(async () => {
            if (!station) {
              setLocalId("");
              setFi("");
              setEn("");
              setLongitude("");
              setLatitude("");
              setNote("");
            }
            await onSaved();
          })
          // The gateway's refusal is the message: it names what the writer is missing.
          .catch((cause: unknown) => setError(problem(cause)))
          .finally(() => setSaving(false));
      }}
    >
      <p className="identity">{MEASURED}</p>
      {!station && (
        <>
          <label htmlFor={field("id")}>Station id</label>
          <input
            id={field("id")}
            required
            pattern="[A-Za-z0-9_-]{1,64}"
            maxLength={64}
            value={localId}
            onChange={(event) => setLocalId(event.target.value)}
          />
        </>
      )}
      <label htmlFor={field("fi")}>Name in Finnish</label>
      <input id={field("fi")} maxLength={200} value={fi} onChange={(event) => setFi(event.target.value)} />
      <label htmlFor={field("en")}>Name in English</label>
      <input id={field("en")} maxLength={200} value={en} onChange={(event) => setEn(event.target.value)} />
      <label htmlFor={field("lon")}>Longitude</label>
      <input
        id={field("lon")}
        type="number"
        required
        step="any"
        min={-180}
        max={180}
        value={longitude}
        onChange={(event) => setLongitude(event.target.value)}
      />
      <label htmlFor={field("lat")}>Latitude</label>
      <input
        id={field("lat")}
        type="number"
        required
        step="any"
        min={-90}
        max={90}
        value={latitude}
        onChange={(event) => setLatitude(event.target.value)}
      />
      <label htmlFor={field("note")}>Steward note</label>
      <textarea id={field("note")} rows={2} maxLength={500} value={note} onChange={(event) => setNote(event.target.value)} />
      <div className="note-actions">
        <button type="submit" disabled={saving || !complete}>
          {station ? "Save changes" : "Add station"}
        </button>
        {onCancel && (
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
        )}
      </div>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}
