import type { components } from "./schema";

export type ActivityEvent = components["schemas"]["ActivityEvent"];

export type Severity = "info" | "warning" | "error";

/** Every kind the vocabulary holds, in the order a filter offers them (Architecture/09 §6). */
export const ACTIVITY_KINDS = [
  "config.planned",
  "config.applied",
  "config.drifted",
  "change.merged",
  "change.rejected",
  "pipeline.throughput",
  "pipeline.error",
  "pipeline.restarted",
  "pipeline.run",
  "pipeline.source-delete",
  "endpoint.traffic",
  "endpoint.rate-limited",
  "access.denied",
  "access.simulated",
  "access.explored",
  "access.revoked",
  "mcp.tool",
  "agent.answer",
  "federation.forward",
  "federation.error",
  "catalogue.published",
  "person.changed",
  "model.key",
  "assistant.transcript",
  "knowledge.file.uploaded",
  "knowledge.file.removed",
  "knowledge.answers.forgotten",
  "access-request",
  "personal-data",
  "mqtt.capture.deleted",
] as const;

export const ACTIVITY_SOURCES = [
  "reconciler",
  "pipeline",
  "gateway",
  "broker",
  "ckan",
  "portal",
] as const;

/** What a view asks for; the list and the tail take the same thing. */
export interface ActivityQuery {
  space?: string;
  kind?: string;
  source?: string;
  severity?: Severity;
  since?: string;
  object?: string;
  /** Only the calls and changes of one connector, by its id (T-5973). */
  connector?: string;
  limit?: number;
  cursor?: string;
}

/**
 * The tail is the one thing the Portal renders that it did not type: a frame is JSON off an
 * EventSource. An event with no `time`, or a `time` no `Date` can read, reaches
 * `Intl.DateTimeFormat.format`, which throws `RangeError: Invalid time value` — and with the
 * only error boundary around the whole app, that one frame blanks the Portal (T-2427). So a
 * frame is checked before anything renders it, against what the row actually draws: a `time`
 * `Date` can read, and words where words are printed (an object handed to React as a child
 * throws as surely as a bad date). A field nobody draws — `project`, `correlationId` — is not
 * asked for: the feed is scoped by the request it made, and dropping a readable event over a
 * field that never reaches the screen would hide what the person came to see.
 */
export function isActivityEvent(value: unknown): value is ActivityEvent {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const event = value as Record<string, unknown>;
  const said = (field: string) =>
    typeof event[field] === "string" && event[field] !== "";
  return (
    ["kind", "severity", "source", "summary", "time"].every(said) &&
    !Number.isNaN(Date.parse(event.time as string))
  );
}

export function activitySearch(query: ActivityQuery): Record<string, string> {
  return Object.fromEntries(
    Object.entries(query)
      .filter(([, value]) => value !== undefined && value !== "")
      .map(([key, value]) => [key, String(value)]),
  );
}

/**
 * The live tail. The Portal names each event after its kind, so a view subscribes to the kinds
 * it draws; `message` catches the rest. Returns the unsubscribe.
 */
export function subscribeActivity(
  project: string,
  query: ActivityQuery,
  onEvent: (event: ActivityEvent) => void,
): () => void {
  const Source = (globalThis as { EventSource?: typeof EventSource })
    .EventSource;
  if (!Source) {
    return () => {};
  }
  const search = new URLSearchParams(activitySearch(query)).toString();
  const source = new Source(
    `/api/v1/projects/${encodeURIComponent(project)}/activity/stream${search ? `?${search}` : ""}`,
    { withCredentials: true },
  );
  const handle = (message: MessageEvent<string>) => {
    try {
      const event: unknown = JSON.parse(message.data);
      if (isActivityEvent(event)) {
        onEvent(event);
      }
    } catch {
      // A keep-alive comment is not an event.
    }
  };
  for (const kind of [...ACTIVITY_KINDS, "message"]) {
    source.addEventListener(kind, handle as EventListener);
  }
  return () => {
    for (const kind of [...ACTIVITY_KINDS, "message"]) {
      source.removeEventListener(kind, handle as EventListener);
    }
    source.close();
  };
}

/**
 * The tail in front of the page it is tailing: newest first, no event twice, and never more
 * than the window a person can read. The identity of an event is its time and its summary —
 * the store hands out no id, and two events of the same kind a minute apart are two events.
 */
export const TAIL_WINDOW = 200;

export function mergeActivity(
  known: ActivityEvent[],
  arriving: ActivityEvent[],
): ActivityEvent[] {
  const readable = known.filter(isActivityEvent);
  const seen = new Set(readable.map(identity));
  const fresh = arriving.filter(
    (event) => isActivityEvent(event) && !seen.has(identity(event)),
  );
  if (fresh.length === 0) {
    return readable.length === known.length ? known : readable;
  }
  return [...fresh, ...readable]
    .sort((a, b) => b.time.localeCompare(a.time))
    .slice(0, TAIL_WINDOW);
}

function identity(event: ActivityEvent): string {
  return `${event.time}|${event.kind}|${event.summary}|${event.space ?? ""}`;
}

/**
 * A DNS-1123 label, which is what both halves of `{plural}/{name}` are (PF-09).
 *
 * The route this builds is `/projects/{project}/{plural}/{name}`, so nothing else may appear in
 * either half. It used to be enough for `details.object` — a value the server writes into the
 * event, not one the Portal computed — to contain a `/`, and the link was built by
 * interpolating it whole. A name carrying `?`, `#` or a space then sent the click to another
 * route, or to this one with search parameters somebody else chose (UI-16, PF-50).
 */
const LABEL = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;

/** The Change a `change.merged` or `change.rejected` event names (T-3292, T-5613), when it is a change id. */
export function changeOf(event: ActivityEvent): string | undefined {
  const details = event.details as Record<string, unknown> | undefined;
  const change = details?.change;
  return (event.kind === "change.merged" || event.kind === "change.rejected") &&
    typeof change === "string" &&
    /^chg-(org-)?[0-9a-f]{8}$/.test(change)
    ? change
    : undefined;
}

/** The object page an event opens, as `{plural}/{name}`, or nothing when it is not one. */
export function objectOf(event: ActivityEvent): string | undefined {
  const details = event.details as Record<string, unknown> | undefined;
  const object = details?.object;
  if (typeof object !== "string") return undefined;
  const parts = object.split("/");
  if (parts.length !== 2) return undefined;
  return parts.every((part) => part.length <= 63 && LABEL.test(part))
    ? object
    : undefined;
}
