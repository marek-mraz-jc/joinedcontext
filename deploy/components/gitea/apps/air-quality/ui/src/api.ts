/**
 * The app's own backend, and nothing else. The endpoint URL never reaches the browser: the
 * server holds it, forwards the user's token to it and answers with what came back (AP-04).
 */
const BASE = import.meta.env.BASE_URL;

export interface Identity {
  signedIn: boolean;
  email: string | null;
  user: string | null;
  anonymous: boolean;
  /** The caller's roles in this App, from the Portal's `/me` (AP-109). */
  roles: string[];
}

export interface Station {
  id: string;
  name?: string;
  /** The name per language, for the form. */
  names?: Record<string, string>;
  pm10?: number;
  pm25?: number;
  airQualityIndex?: number;
  stewardNote?: string;
  observedAt?: string;
  coordinates?: [number, number];
  /** Added by a steward rather than a pipeline, so a steward may remove it. */
  own?: boolean;
}

/** What the record form writes: the station attributes a person may correct, nothing measured. */
export interface StationFields {
  localId?: string;
  name?: Record<string, string>;
  coordinates?: [number, number];
  stewardNote?: string;
}

/** A refusal carries the gateway's own words, which is what the user needs to read. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly detail: string,
  ) {
    super(detail);
  }
}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${BASE}api/${path}`, {
    ...init,
    headers: { "Content-Type": "application/json", ...(init?.headers ?? {}) },
  });
  if (!response.ok) {
    const problem = (await response.json().catch(() => null)) as { detail?: string } | null;
    throw new ApiError(response.status, problem?.detail ?? response.statusText);
  }
  return response.status === 204 ? (undefined as T) : ((await response.json()) as T);
}

export const getIdentity = (): Promise<Identity> => call<Identity>("me");
export const getStations = (): Promise<Station[]> => call<Station[]>("stations");
const station = (id: string): string => `stations/${encodeURIComponent(id)}`;
export const createStation = (fields: StationFields): Promise<{ id: string }> =>
  call<{ id: string }>("stations", { method: "POST", body: JSON.stringify(fields) });
export const updateStation = (id: string, fields: StationFields): Promise<void> =>
  call<void>(station(id), { method: "PATCH", body: JSON.stringify(fields) });
export const deleteStation = (id: string): Promise<void> =>
  call<void>(station(id), { method: "DELETE" });
/** One day of the station's PM10 and PM2.5, the temporal entity as the endpoint answered it. */
export const getHistory = (id: string): Promise<unknown> => call<unknown>(`${station(id)}/history`);
