import { useId, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized, refName } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { fetchJson, publishedTypes } from "../endpoints/SchemaProjectionPanel";
import type { PublishedType } from "../endpoints/SchemaProjectionPanel";
import { EndpointPreview } from "./EndpointPreview";
import { NewEndpointPanel } from "./NewEndpointPanel";
import { useAccess } from "../../components/entities/AccessPanel";
import type { GrantDocument } from "../../api/access";
import {
  Alert,
  Button,
  Checkbox,
  ExternalLink,
  Field,
  InlineError,
  Input,
  PageHeader,
  Select,
  Textarea,
} from "../../components/ui";
import { guideUrl, useBranding } from "../../branding";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { takePrefill } from "../../assistant/state";
import { kindPlural } from "../../kindWords";

/** The blueprint that turns a description into an app (AP-22, Architecture/16 §3). */
export const BLUEPRINT = "app-from-prompt";

/**
 * `spec.kind` of the App the blueprint writes (AP-124). `ui` is the kit pass, a dashboard inside a
 * minute from one model call (AP-56); `ui-rust` is what the workspace builds (AP-25); `wasm` is the
 * kit pass with a Rust server component on the shared host and a table of its own (T-3575).
 * `ui-node` is not offered until the shape is built (AP-125).
 */
export const APP_KINDS = ["ui", "ui-rust", "wasm"] as const;
export type AppKind = (typeof APP_KINDS)[number];

/** The two apps that ship with the platform, for a deployment with no builder to point at. */
export const EXAMPLE_APPS = ["hsl-transport", "air-quality"] as const;

/** How many endpoints one application may read (AP-44), the first one the primary. */
export const MAX_ENDPOINTS = 5;

/** The User Guide page of apps, linked from the builder when the installation serves a guide. */
const GUIDE = "User-Guide/11-apps";

/** The published model of one endpoint, read with the person's own session. */
export async function endpointSchema(slug: string): Promise<unknown> {
  // The slug comes off a manifest and goes straight into a path: a `/` or a `..` in it would
  // retarget the request at another endpoint's surface (T-1760).
  const base = `${window.location.origin}/api/endpoint/${encodeURIComponent(slug)}/schema`;
  const index = (await fetchJson(`${base}/index.json`)) as { models?: { version?: number }[] };
  return fetchJson(`${base}/v${index.models?.[0]?.version ?? 1}/json-schema`);
}

/**
 * The request's endpoints (AP-44): `endpointName` for one, `endpointNames` with the primary first
 * when the person added more.
 */
export function endpointFields(primary: string, extra: string[]): Record<string, unknown> {
  const others = extra.filter((name) => name !== "" && name !== primary);
  return others.length === 0 ? { endpointName: primary } : { endpointNames: [primary, ...others] };
}

/** What every generated app reads with, whatever else its preset adds. */
const OPERATIONS = ["queryEntity", "retrieveEntity"];

/**
 * The three access presets of Build an app (AP-132): what the app may do on its endpoint. A write
 * preset is offered only where the person's own grant holds every write it adds (PF-70).
 */
export const ACCESS_PRESETS = {
  read: [...OPERATIONS, "queryTemporal", "retrieveTemporal"],
  update: [...OPERATIONS, "queryTemporal", "retrieveTemporal", "updateAttrs", "appendAttrs"],
  full: [
    ...OPERATIONS,
    "queryTemporal",
    "retrieveTemporal",
    "updateAttrs",
    "appendAttrs",
    "createEntity",
    "deleteEntity",
  ],
} as const;
export type Preset = keyof typeof ACCESS_PRESETS;
export const PRESETS = Object.keys(ACCESS_PRESETS) as Preset[];

const TEMPORAL_READS = new Set(["queryTemporal", "retrieveTemporal"]);
const isRead = (operation: string) => OPERATIONS.includes(operation) || TEMPORAL_READS.has(operation);

/** The operations the person holds on one type, as the endpoint's `/access` document states them. */
export function heldOperations(document: GrantDocument | undefined, type: string): Set<string> {
  const on = (entries: GrantDocument["permissions"]) =>
    (entries ?? [])
      .filter((entry) => entry.resource?.type === type || entry.resource?.type === "*")
      .flatMap((entry) => entry.actions ?? []);
  const prohibited = new Set(on(document?.prohibitions));
  return new Set(on(document?.permissions).filter((operation) => !prohibited.has(operation)));
}

/** Whether the person holds every write of `preset` on every one of `types` (PF-70). */
export function offersPreset(preset: Preset, document: GrantDocument | undefined, types: string[]): boolean {
  const writes = ACCESS_PRESETS[preset].filter((operation) => !isRead(operation));
  return types.length > 0 && types.every((type) => {
    const held = heldOperations(document, type);
    return writes.every((operation) => held.has(operation));
  });
}

/**
 * What a need of `preset` carries: the two reads every app makes, the temporal reads where the
 * person holds them on every type, and the preset's writes. The server checks the same list
 * against the same document (AP-132).
 */
export function presetOperations(preset: Preset, document: GrantDocument | undefined, types: string[]): string[] {
  return ACCESS_PRESETS[preset].filter(
    (operation) =>
      OPERATIONS.includes(operation) ||
      !TEMPORAL_READS.has(operation) ||
      (types.length > 0 && types.every((type) => heldOperations(document, type).has(operation))),
  );
}

/** Words that say what to do, not what the app is: a name made of them says nothing. */
const FILLER = new Set(
  (
    "create generate make build show give add want need please can could would like let " +
    "me us i we you a an the of for with and or on in at to from by that which this these new " +
    "my our your some all app apps application applications dashboard page site web " +
    "vytvor vygeneruj urob sprav ukaz pridaj chcem potrebujem prosim novu nove novy " +
    "vytvorit udelej ukaz chci potrebuji prosim " +
    "erstelle erzeuge mach zeige gib fuge ich will brauche bitte eine einen ein der die das " +
    "fur mit und von im"
  ).split(" "),
);

/**
 * A name for the app, from what the person asked for.
 *
 * It becomes the path the app is served at, so it is the lower-case, dash-joined form a URL
 * takes, made of the first three words that say what the app is ("Create a map of the bike
 * stations" is `map-bike-stations`, not `create-a-map-of`). With nothing left it is the
 * endpoint's app; a person who wants another name writes it under the details.
 */
/** What an app name may be: one DNS-1123 label, which is what a URL segment and a router param take. */
export const APP_NAME = /^[a-z0-9]([a-z0-9-]{0,38}[a-z0-9])?$/;

export function slugOf(prompt: string, endpointName = ""): string {
  const words = prompt
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((word) => word !== "" && !FILLER.has(word))
    .slice(0, 3);
  const fallback = `${endpointName}-app`.replace(/^-+/, "");
  const slug = (words.length > 0 ? words.join("-") : fallback).slice(0, 40).replace(/-+$/, "");
  return slug === "" ? "app" : slug;
}

interface EndpointSpec {
  slug?: string;
  /** A `Ref`: a bare name, or `{ kind, name }`. Read it with `refName`, never as a string. */
  contextSpaceRef?: unknown;
  audience?: string;
  enabledRepresentations?: string[];
}

export function endpointSpec(endpoint: Manifest): EndpointSpec {
  return endpoint.spec as EndpointSpec;
}

/**
 * What the app would be allowed to read, derived from what the Endpoint actually publishes.
 *
 * The types and attributes come from the endpoint's own schema surface, which is already the
 * projection its Policy allows, so nothing here can name an attribute the endpoint hides. The
 * user unticks what the app does not need; there is no control that adds one back, which is
 * what AP-22 means by the confirmed list being the grant rather than the prompt.
 */
/**
 * The types an app can read: the schema's definitions minus the base classes a model derives
 * from, which no entity is served as (`Entity`, or a definition the schema marks abstract).
 */
export function concreteTypes(document: unknown): ReturnType<typeof publishedTypes> {
  const doc = document as {
    $defs?: Record<string, { abstract?: unknown }>;
    definitions?: Record<string, { abstract?: unknown }>;
  } | null;
  const defs = { ...doc?.$defs, ...doc?.definitions };
  return publishedTypes(document).filter(
    (type) => type.name !== "Entity" && defs[type.name]?.abstract !== true,
  );
}

export function dataNeeds(
  endpoint: Manifest,
  types: PublishedType[],
  dropped: string[],
  operations: readonly string[] = OPERATIONS,
  writeRole = "",
): Record<string, unknown>[] {
  const spec = endpointSpec(endpoint);
  const writes = operations.some((operation) => !isRead(operation));
  const kept = types
    .map((type) => ({
      name: type.name,
      attributes: type.attributes.filter((attr) => !dropped.includes(`${type.name}.${attr}`)),
    }))
    .filter((type) => type.attributes.length > 0);
  if (kept.length === 0) {
    return [];
  }
  // Exactly the fields of jc-core's `DataNeed`: this value becomes `spec.dataNeeds` of the App
  // manifest the run publishes, and that kind refuses a field it does not know.
  const need = {
    contextSpaceRef: { kind: "ContextSpace", name: refName(spec.contextSpaceRef) },
    types: kept.map((type) => type.name),
    attrs: [...new Set(kept.flatMap((type) => type.attributes))].sort(),
    operations: writes && writeRole === "" ? [...operations] : operations.filter(isRead),
    representations: spec.enabledRepresentations ?? [],
  };
  // Everyone the app admits reads; only the named application role writes. Publishing declares
  // the role in the App, and its members are added on the App page (AP-91, AP-96).
  return writes && writeRole !== "" ? [need, { ...need, operations: [...operations], roles: [writeRole] }] : [need];
}

/** `[a-z][a-z0-9-]{0,31}`, an application role name as the App kind takes it (AP-90). */
export function isRoleName(name: string): boolean {
  return /^[a-z][a-z0-9-]{0,31}$/.test(name);
}

/**
 * What an app of these endpoints would be allowed to read and write (AP-22, AP-44, AP-132): the
 * primary endpoint's published types less what the person unticked, with the preset's operations
 * where the person's own grant holds them, else reading; every further endpoint read whole. The
 * builder's form and the assistant's Build card (T-2721) start a run from the same answer.
 */
export function useAppNeeds(
  project: string,
  endpointName: string,
  extra: string[],
  preset: Preset,
  dropped: string[] = [],
  writeRole = "",
) {
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });

  const choices = asManifests(endpoints.data?.items ?? []);
  const endpoint = choices.find((candidate) => candidate.metadata.name === endpointName);
  const slug = endpoint ? (endpointSpec(endpoint).slug ?? "") : "";
  // The option to update exists only where the person's own grant on the endpoint has a write
  // (AP-22, AP-62): the gateway evaluates each save anyway; this keeps the form honest.
  const access = useAccess(slug === "" ? undefined : slug);

  // The endpoint's published model, which is where the app's bounds come from.
  const schema = useQuery({
    queryKey: ["generator-endpoint-schema", slug],
    enabled: slug !== "",
    retry: false,
    queryFn: async () => endpointSchema(slug),
  });

  // Every added endpoint is read whole: all its concrete types and attributes, never written.
  const extraEndpoints = extra
    .filter((name) => name !== endpointName)
    .map((name) => choices.find((candidate) => candidate.metadata.name === name))
    .filter((candidate): candidate is Manifest => candidate !== undefined);
  const extraSchemas = useQueries({
    queries: extraEndpoints.map((candidate) => {
      const extraSlug = endpointSpec(candidate).slug ?? "";
      return {
        queryKey: ["generator-endpoint-schema", extraSlug],
        enabled: extraSlug !== "",
        retry: false,
        queryFn: async () => endpointSchema(extraSlug),
      };
    }),
  });

  // Both are a pass over a handful of names; the React Compiler memoizes them, and a manual
  // useMemo here only tells it a dependency might be mutated when none of them is.
  const types = concreteTypes(schema.data);
  const kept = types
    .filter((type) => type.attributes.some((attribute) => !dropped.includes(`${type.name}.${attribute}`)))
    .map((type) => type.name);
  // A preset the grant no longer carries (another endpoint picked, a type unticked) falls back to
  // reading, never to a write the person does not hold.
  const chosenPreset = offersPreset(preset, access.data, kept) ? preset : "read";
  const needs = endpoint
    ? [
        ...dataNeeds(
          endpoint,
          types,
          dropped,
          presetOperations(chosenPreset, access.data, kept),
          chosenPreset === "read" ? "" : writeRole.trim(),
        ),
        ...extraEndpoints.flatMap((candidate, i) =>
          dataNeeds(candidate, concreteTypes(extraSchemas[i]?.data), []),
        ),
      ]
    : [];
  return { endpoints, choices, endpoint, slug, access, schema, extraEndpoints, extraSchemas, types, kept, chosenPreset, needs };
}

/**
 * "Generate your own app" (AP-22, AP-30, AP-51, AG-26, AG-43).
 *
 * The form is the whole of what a person has to decide: which endpoint, what the app should do,
 * and which of the attributes the endpoint publishes it may read. Submitting it starts an agent
 * run (`POST …/agent-runs`), and the answer is a run to watch rather than a merge request to
 * wait for: the review comes at the end, when the person publishes what was built (AP-55).
 *
 * A deployment without the Agent Runner has no `app-from-prompt` blueprint to offer, which is
 * what the banner below says rather than a button that leads nowhere (ADR-N-014).
 */
export function AppGenerator({
  project,
  initialName,
  initialPrompt,
  onStarted,
  embedded = false,
}: {
  project: string;
  initialName?: string;
  /** Inside another page whose H1 names the App: the generator's heading is an H2 (T-4264). */
  embedded?: boolean;
  /** What the app should do, already written: a template's purpose naming the template (T-3263). */
  initialPrompt?: string;
  /** Called with the new run before the page moves to the app, so the assistant can follow it. */
  onStarted?: (runId: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const guideHref = guideUrl(useBranding(), GUIDE);
  const [name, setName] = useState(initialName ?? "");
  const [kind, setKind] = useState<AppKind>("ui");
  const [prompt, setPrompt] = useState(initialPrompt ?? "");
  // The endpoints the assistant's "Build an app" path handed over (T-2696), taken once: the first
  // is the app's endpoint, the rest are read beside it.
  const [handed] = useState(() => {
    const endpoints = takePrefill(`/projects/${project}/apps/new`)?.endpoints;
    return Array.isArray(endpoints) ? endpoints.filter((name): name is string => typeof name === "string") : [];
  });
  const [endpointName, setEndpointName] = useState(handed[0] ?? "");
  /** Endpoints read beside the primary one, e.g. an indicator space's (AP-44). */
  const [extra, setExtra] = useState<string[]>(handed.slice(1));
  const [addingEndpoint, setAddingEndpoint] = useState(false);
  /** "New endpoint" is open: proposed as its own Change, read once it is served (AP-132). */
  const [creatingEndpoint, setCreatingEndpoint] = useState(false);
  const [dropped, setDropped] = useState<string[]>([]);
  const [preset, setPreset] = useState<Preset>("read");
  /** Empty: everyone the app admits may write. A name: only that application role (AP-96). */
  const [writeRole, setWriteRole] = useState("");
  const [conflictApp, setConflictApp] = useState<string | null>(null);
  // Mounted twice at once — the dock renders one over the apps page's own (AssistantDock:448,
  // AppPage:67) — and the ids were the same string in both, so a label click focused the control
  // behind it (T-1760).
  const base = useId();
  const ids = {
    endpoint: `${base}-endpoint`,
    prompt: `${base}-prompt`,
    name: `${base}-name`,
    kind: `${base}-kind`,
    writeRole: `${base}-write-role`,
    preset: `${base}-preset`,
  };
  const starting = useRef(false);
  const [change, setChange] = useState<Change | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** One entry per rule the parameters broke, so every bad field is named at once (CC-24). */
  const [violations, setViolations] = useState<string[]>([]);

  const blueprints = useQuery({
    queryKey: ["blueprints"],
    queryFn: async () => unwrap(await api.GET("/api/v1/blueprints", {})),
  });

  const { endpoints, choices, endpoint, slug, access, schema, extraEndpoints, types, kept, chosenPreset, needs } = useAppNeeds(
    project,
    endpointName,
    extra,
    preset,
    dropped,
    writeRole,
  );
  const available = asManifests(blueprints.data?.items ?? []).find(
    (blueprint) => blueprint.metadata.name === BLUEPRINT,
  );

  const generate = useMutation({
    mutationFn: async () => {
      setError(null);
      setViolations([]);
      setConflictApp(null);
      return unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project } },
          body: {
            appName: chosen,
            appClass: kind,
            ...endpointFields(endpointName, extraEndpoints.map((candidate) => candidate.metadata.name)),
            prompt,
            // The confirmed list, derived from the endpoint: the run is refused if it names
            // anything the endpoint does not publish, so the two cannot drift (AP-44).
            dataNeeds: needs,
          },
        }),
      );
    },
    onSuccess: (result) => {
      const created = result as unknown as { id?: string; appName?: string };
      const targetName = created.appName || chosen;
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "agent-runs"] });
      if (typeof created.id === "string") {
        onStarted?.(created.id);
        void navigate({
          to: "/projects/$project/$plural/$name",
          params: { plural: "apps", project, name: targetName },
        });
        return;
      }
      if (isChange(result)) {
        setChange(result);
      }
    },
    onSettled: () => {
      starting.current = false;
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        if (err.status === 409) {
          setConflictApp(chosen);
        }
        setViolations(err.problem?.errors ?? []);
        setError(err.problem?.detail ?? err.message);
        return;
      }
      setError(t("app.error.generic"));
    },
  });

  if (blueprints.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }

  // A failed read is not "this deployment has no app builder": a person told that stops trying,
  // and what happened was a 500 they could have retried (T-1760).
  if (blueprints.isError) {
    return (
      <Alert
        tone="danger"
        actions={
          <Button variant="secondary" size="sm" onClick={() => void blueprints.refetch()}>
            {t("form.listRetry")}
          </Button>
        }
      >
        {t("form.listFailed", {
          reason:
            blueprints.error instanceof ApiError
              ? (blueprints.error.problem?.detail ?? blueprints.error.message)
              : t("app.error.generic"),
        })}
      </Alert>
    );
  }

  if (!available) {
    return <NoBuilder embedded={embedded} />;
  }

  // A name is needed for the URL the app is served at, not for the conversation: it is derived
  // from what the person asked for and stays editable under the details.
  const chosen = name.trim() === "" ? slugOf(prompt, endpointName) : name.trim();
  // `slugOf` sanitises only the *derived* name. What a person types goes into `appName`, into
  // `/projects/$project/apps/$name` and into the served URL, so spaces, slashes, upper case and
  // two hundred characters all used to travel (T-1760).
  const nameFault = APP_NAME.test(chosen) ? null : "apps.generate.nameInvalid";
  // The first thing still missing, named, so a new user is told what the grey button waits for
  // instead of guessing (T-3214).
  const missing =
    endpointName === ""
      ? "apps.generate.missing.endpoint"
      : prompt.trim() === ""
        ? "apps.generate.missing.prompt"
        : nameFault !== null || chosen === ""
          ? "apps.generate.missing.name"
          : needs.length === 0
            ? "apps.generate.missing.needs"
            : null;
  const ready = missing === null;

  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        // `generate.isPending` reaches the button one render later than a second click does,
        // and two clicks started two runs — two applications, two agents, two bills (T-1760).
        if (!ready || starting.current || generate.isPending) return;
        starting.current = true;
        generate.mutate();
      }}
    >
      <GeneratorHeading embedded={embedded} />
      {guideHref ? (
        <p className="text-body">
          <ExternalLink data-testid="form-guide" href={guideHref} className="text-primary-soft-fg underline-offset-2">
            {t("form.guideLink", { kind: kindPlural("App", t) })}
          </ExternalLink>
        </p>
      ) : null}

      {change && <ChangeNotice change={change} project={project} />}
      {error && (
        <Alert tone="danger">
          <p>{error}</p>
          {conflictApp && (
            <p className="mt-1 text-sm">
              <Link
                to="/projects/$project/$plural/$name"
                params={{ plural: "apps", project, name: conflictApp }}
                className="underline hover:no-underline"
              >
                {t("apps.drafts.conflict", { name: conflictApp })}
              </Link>
            </p>
          )}
        </Alert>
      )}
      {violations.length > 0 && (
        <Alert tone="danger">
          <ul className="list-disc pl-5 text-sm">
            {violations.map((violation) => (
              <li key={violation}>{violation}</li>
            ))}
          </ul>
        </Alert>
      )}

      <div>
        <Field
          id={ids.endpoint}
          label={t("apps.generate.endpoint")}
          help={
            endpoints.isPending
              ? t("app.loading")
              : !endpoints.isError && choices.length === 0
                ? t("apps.generate.noEndpoints")
                : t("apps.generate.endpointHint")
          }
          errors={
            endpoints.isError
              ? [
                  t("form.listFailed", {
                    reason:
                      endpoints.error instanceof ApiError
                        ? (endpoints.error.problem?.detail ?? endpoints.error.message)
                        : t("app.error.generic"),
                  }),
                ]
              : undefined
          }
        >
        <Select
          id={ids.endpoint}
          value={endpointName}
          onChange={(event) => {
            setEndpointName(event.target.value);
            setExtra((current) => current.filter((name) => name !== event.target.value));
            setDropped([]);
            setPreset("read");
          }}
          className="mt-1"
        >
          <option value="">{t("apps.generate.pickEndpoint")}</option>
          {choices.map((candidate) => (
            <option key={candidate.metadata.name} value={candidate.metadata.name}>
              {localized(candidate.metadata.title, i18n.language, candidate.metadata.name)}
            </option>
          ))}
        </Select>
        </Field>
        {creatingEndpoint ? (
          <NewEndpointPanel
            project={project}
            onCancel={() => setCreatingEndpoint(false)}
            onLive={(created, wanted) => {
              setCreatingEndpoint(false);
              // Only the primary endpoint is written, so a write preset makes the new one primary
              // and the one before it is read beside it. The access select still offers the
              // preset only where the grant holds it (PF-70).
              if (endpointName === "" || wanted !== "read") {
                if (endpointName !== "") {
                  setExtra((current) => [endpointName, ...current.filter((n) => n !== created)].slice(0, MAX_ENDPOINTS - 1));
                  setAddingEndpoint(true);
                }
                setEndpointName(created);
                setDropped([]);
                setPreset(wanted);
              } else {
                setExtra((current) =>
                  current.includes(created) || current.length + 1 >= MAX_ENDPOINTS ? current : [...current, created],
                );
                setAddingEndpoint(true);
              }
            }}
          />
        ) : (
          <Button variant="ghost" size="sm" className="mt-2" onClick={() => setCreatingEndpoint(true)}>
            {t("apps.generate.newEndpoint.open")}
          </Button>
        )}
        {endpointName !== "" && choices.length > 1 && (
          <div className="mt-2">
            {!addingEndpoint && extra.length === 0 ? (
              <Button variant="ghost" size="sm" onClick={() => setAddingEndpoint(true)}>
                {t("apps.generate.addEndpoint")}
              </Button>
            ) : (
              <fieldset>
                <legend className="text-sm font-medium">{t("apps.generate.moreEndpoints")}</legend>
                <p className="text-xs text-fg-muted">{t("apps.generate.moreEndpointsHint")}</p>
                <ul className="mt-1 grid gap-1 sm:grid-cols-2">
                  {choices
                    .filter((candidate) => candidate.metadata.name !== endpointName)
                    .map((candidate) => {
                      const candidateName = candidate.metadata.name;
                      const checked = extra.includes(candidateName);
                      return (
                        <li key={candidateName}>
                          <Checkbox
                            label={localized(
                              candidate.metadata.title,
                              i18n.language,
                              candidateName,
                            )}
                            checked={checked}
                            disabled={!checked && extra.length + 1 >= MAX_ENDPOINTS}
                            disabledReason={t("apps.generate.moreEndpointsFull", {
                              count: MAX_ENDPOINTS,
                            })}
                            onChange={(event) => {
                              setExtra((current) =>
                                event.target.checked
                                  ? [...current, candidateName]
                                  : current.filter((name) => name !== candidateName),
                              );
                            }}
                          />
                        </li>
                      );
                    })}
                </ul>
              </fieldset>
            )}
          </div>
        )}
      </div>

      {/* What the endpoint gives you, read with your own session, before you describe the app. */}
      {slug !== "" && <EndpointPreview slug={slug} />}

      {/*
        The description is the whole brief, and the first turn of a conversation rather than a
        specification: the agent builds from it, shows what it built, and is told what to change
        next on the run page.
      */}
      <Field id={ids.prompt} label={t("apps.generate.prompt")} help={t("apps.generate.promptHint")} required>
        <Textarea
          id={ids.prompt}
          rows={5}
          value={prompt}
          placeholder={t("apps.generate.promptPlaceholder")}
          onChange={(event) => {
            setPrompt(event.target.value);
          }}
          className="mt-1"
        />
      </Field>

      <details className="rounded-md border border-border px-4 py-2">
        <summary className="cursor-pointer text-sm font-medium">
          {t("apps.generate.details", { name: chosen === "" ? "…" : chosen })}
        </summary>

        <div className="mt-3 space-y-4">
          <Field
            id={ids.name}
            label={t("apps.generate.name")}
            help={t("apps.generate.nameHint")}
            errors={nameFault ? [t(nameFault)] : undefined}
          >
            <Input
              id={ids.name}
              value={name}
              placeholder={slugOf(prompt, endpointName)}
              onChange={(event) => {
                setName(event.target.value);
              }}
              className="mt-1"
            />
          </Field>

          <Field id={ids.kind} label={t("apps.generate.kind")}>
            <Select
              id={ids.kind}
              value={kind}
              onChange={(event) => {
                setKind(event.target.value as AppKind);
              }}
              className="mt-1"
            >
              {APP_KINDS.map((value) => (
                <option key={value} value={value}>
                  {t(`apps.generate.kinds.${value}`)}
                </option>
              ))}
            </Select>
          </Field>

          {endpointName !== "" && (
            <NeedsChecklist
              audience={endpoint ? (endpointSpec(endpoint).audience ?? "") : ""}
              types={types}
              dropped={dropped}
              preset={chosenPreset}
              offered={PRESETS.filter((candidate) => offersPreset(candidate, access.data, kept))}
              presetId={ids.preset}
              onPreset={setPreset}
              writeRole={writeRole}
              writeRoleId={ids.writeRole}
              onWriteRole={setWriteRole}
              state={schema.isPending ? "loading" : schema.isError ? "unavailable" : "ready"}
              onToggle={(attribute) => {
                setDropped((current) =>
                  current.includes(attribute)
                    ? current.filter((name) => name !== attribute)
                    : [...current, attribute],
                );
              }}
            />
          )}
        </div>
      </details>

      {/* The run proposes an App in this project, so that is the verb it needs. Without the
          guard a viewer wrote the whole brief, curated the checklist, pressed the button and
          met a raw 403 (UI-44, T-1760). */}
      <PermissionGuard project={project} kind="App" verb="propose">
        <Button
          type="submit"
          variant="primary"
          loading={generate.isPending}
          disabled={!ready}
          disabledReason={missing ? t(missing) : undefined}
        >
          {t("apps.generate.submit")}
        </Button>
      </PermissionGuard>
    </form>
  );
}

/** The derived needs, as the checklist AP-22 asks for. */
function NeedsChecklist({
  audience,
  types,
  dropped,
  preset,
  offered,
  presetId,
  onPreset,
  writeRole,
  writeRoleId,
  onWriteRole,
  state,
  onToggle,
}: {
  audience: string;
  types: PublishedType[];
  dropped: string[];
  preset: Preset;
  offered: Preset[];
  presetId: string;
  onPreset: (preset: Preset) => void;
  writeRole: string;
  writeRoleId: string;
  onWriteRole: (role: string) => void;
  state: "loading" | "unavailable" | "ready";
  onToggle: (attribute: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="generator-needs" className="space-y-2 rounded-md border border-border p-4">
      <h2 id="generator-needs" className="text-base font-semibold">
        {t("apps.generate.needs.title")}
      </h2>
      <p className="text-sm text-fg-muted">{t("apps.generate.needs.hint")}</p>
      {audience !== "" && (
        <p className="text-sm text-fg-muted">{t("apps.generate.needs.audience", { audience })}</p>
      )}
      <p className="text-sm text-fg-muted">{t("apps.generate.needs.loginOnly")}</p>
      <Field
        id={presetId}
        label={t("apps.generate.needs.access")}
        help={
          offered.length < PRESETS.length
            ? t("apps.generate.needs.accessHeld")
            : t("apps.generate.needs.accessHelp")
        }
      >
        <Select
          id={presetId}
          value={preset}
          onChange={(event) => {
            onPreset(event.target.value as Preset);
          }}
        >
          {PRESETS.map((candidate) => (
            <option key={candidate} value={candidate} disabled={!offered.includes(candidate)}>
              {t(`apps.generate.needs.presets.${candidate}`)}
            </option>
          ))}
        </Select>
      </Field>
      {preset !== "read" && (
        <Field
          id={writeRoleId}
          label={t("apps.generate.needs.writeRole")}
          help={t("apps.generate.needs.writeRoleHelp")}
          errors={
            writeRole.trim() === "" || isRoleName(writeRole.trim())
              ? undefined
              : [t("apps.generate.needs.writeRoleInvalid")]
          }
        >
          <Input
            id={writeRoleId}
            value={writeRole}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => {
              onWriteRole(event.target.value);
            }}
          />
        </Field>
      )}
      {state === "loading" && <p role="status">{t("apps.generate.needs.loading")}</p>}
      {state === "unavailable" && (
        <InlineError>
          {t("apps.generate.needs.unavailable")}
        </InlineError>
      )}
      {state === "ready" && types.length === 0 && <p>{t("apps.generate.needs.none")}</p>}
      {types.map((type) => (
        <fieldset key={type.name} className="mt-2">
          <legend className="text-sm font-medium">{type.name}</legend>
          <div className="mt-1 flex flex-wrap gap-3">
            {type.attributes.map((attribute) => {
              const id = `${type.name}.${attribute}`;
              return (
                <Checkbox
                  key={id}
                  label={attribute}
                  checked={!dropped.includes(id)}
                  onChange={() => {
                    onToggle(id);
                  }}
                />
              );
            })}
          </div>
        </fieldset>
      ))}
    </section>
  );
}

/**
 * A deployment without the Agent Runner (ADR-N-014 makes it optional). Saying so beats a
 * button that opens a merge request nothing will ever pick up, and the reference apps are
 * the worked examples a reader wanted anyway.
 */
function NoBuilder({ embedded }: { embedded: boolean }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="space-y-3">
      <GeneratorHeading embedded={embedded} />
      <Alert tone="info">{t("apps.generate.noBuilder")}</Alert>
      <p className="text-sm">{t("apps.generate.examplesHint")}</p>
      <ul className="list-disc pl-5 text-sm">
        {EXAMPLE_APPS.map((app) => (
          <li key={app}>
            <a
              href={`/apps/${app}/`}
              className="text-primary-soft-fg underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus"
            >
              {t(`apps.generate.examples.${app}`)}
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** The generator's own page header, or an H2 under the page of the App it edits. */
function GeneratorHeading({ embedded }: { embedded: boolean }): JSX.Element {
  const { t } = useTranslation();
  if (!embedded) return <PageHeader title={t("apps.generate.title")} description={t("apps.generate.subtitle")} />;
  return (
    <div className="space-y-1">
      <h2 className="font-heading text-title font-semibold">{t("apps.generate.title")}</h2>
      <p className="text-body text-fg-muted">{t("apps.generate.subtitle")}</p>
    </div>
  );
}
