import { useEffect, useId, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests } from "../../api/manifest";
import type { Change, ResourceProposal } from "../../api/manifest";
import { useProposal } from "../../api/proposal";
import { ChangeNotice } from "../../components/ChangeNotice";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { TypePicker } from "../../components/pickers/TypePicker";
import { Alert, Button, Field, Input, RadioGroup } from "../../components/ui";
import { DNS1123 } from "../../schemas/kinds";

/** The presets of AP-132, in the order the builder offers them. */
export const NEW_ENDPOINT_PRESETS = ["read", "update", "full"] as const;
export type NewEndpointPreset = (typeof NEW_ENDPOINT_PRESETS)[number];

/** How often the builder asks whether the proposed endpoint is served yet. */
const LIVE_POLL_MS = 5000;

/** What `propose-endpoint` renders with `access` (API/04 §8): nothing is written by it. */
interface Rendering {
  lane: string;
  endpoint: ResourceProposal;
  policies: ResourceProposal[];
  groups?: ResourceProposal[];
}

/** The request the panel sends: this project's own endpoint, and the types a write preset changes. */
export function newEndpointRequest(
  name: string,
  space: string,
  preset: NewEndpointPreset,
  types: string[],
): Record<string, unknown> {
  return {
    contextSpace: space,
    name,
    access: preset,
    ...(preset === "read" ? {} : { entityTypes: types }),
  };
}

/** What still stops the proposal, in the order the form asks for it; empty when it may go. */
export function newEndpointMissing(
  name: string,
  space: string,
  preset: NewEndpointPreset,
  types: string[],
): ("name" | "space" | "types")[] {
  const missing: ("name" | "space" | "types")[] = [];
  if (!new RegExp(DNS1123).test(name) || name.length > 63) {
    missing.push("name");
  }
  if (space === "") {
    missing.push("space");
  }
  if (preset !== "read" && types.length === 0) {
    missing.push("types");
  }
  return missing;
}

/**
 * "New endpoint" of Build an app (AP-132): a space, a preset and, for a write, the types it
 * changes. The Portal renders the Endpoint (and, for a write, its one Policy) and the panel
 * proposes it at once as a Change of its own; the run starts on it only once it is served, so the
 * panel waits for the endpoint to reach the project's list and hands it to the builder then.
 */
export function NewEndpointPanel({
  project,
  onLive,
  onCancel,
}: {
  project: string;
  /** The endpoint is served: the builder reads it, with the preset asked for. */
  onLive: (name: string, preset: NewEndpointPreset) => void;
  onCancel: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const base = useId();
  const ids = { name: `${base}-name`, space: `${base}-space`, types: `${base}-types`, preset: `${base}-preset` };
  const [name, setName] = useState("");
  const [space, setSpace] = useState("");
  const [preset, setPreset] = useState<NewEndpointPreset>("read");
  const [types, setTypes] = useState<string[]>([]);
  const [change, setChange] = useState<Change | null>(null);
  const [lane, setLane] = useState<string | null>(null);
  const [renderError, setRenderError] = useState<string | null>(null);
  const [touched, setTouched] = useState(false);
  const missing = newEndpointMissing(name.trim(), space, preset, types);

  const proposal = useProposal(project, "endpoints", setChange);
  const render = useMutation({
    mutationFn: async () => {
      setRenderError(null);
      const rendering = (await unwrap(
        await api.POST("/api/v1/projects/{project}/assistant/propose-endpoint", {
          params: { path: { project } },
          body: newEndpointRequest(name.trim(), space, preset, types) as Record<string, never>,
        }),
      )) as Rendering;
      setLane(rendering.lane);
      await proposal.mutation.mutateAsync({
        body: rendering.endpoint,
        create: true,
        bundle: [...(rendering.groups ?? []), ...rendering.policies],
      });
    },
    onError: (err) => {
      setRenderError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : t("app.error.generic"));
    },
  });

  const propose = (): void => {
    setTouched(true);
    if (missing.length === 0 && !render.isPending) {
      render.mutate();
    }
  };

  // Served once the endpoint is on the project's list: the mirror holds what main holds. The
  // Change is followed too, so a rejection ends the wait instead of polling for ever (F-R2UI2-014).
  const waiting = change !== null;
  const changeId = change?.metadata.name ?? "";
  const followed = useQuery({
    queryKey: queryKeys.change(project, changeId),
    enabled: waiting,
    refetchInterval: (query) => (query.state.data?.status.phase === "Rejected" ? false : LIVE_POLL_MS),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/changes/{id}", { params: { path: { project, id: changeId } } })),
  });
  const phase = followed.data?.status.phase ?? change?.status.phase;
  const holds = (answer: { items?: Parameters<typeof asManifests>[0] } | undefined) =>
    asManifests(answer?.items ?? []).some((endpoint) => endpoint.metadata.name === name.trim());
  const rejected = phase === "Rejected";
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    enabled: waiting && !rejected,
    refetchInterval: (query) => (holds(query.state.data) ? false : LIVE_POLL_MS),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });
  const served = waiting && holds(endpoints.data);
  const handedOver = useRef(false);
  useEffect(() => {
    if (served && !handedOver.current) {
      handedOver.current = true;
      onLive(name.trim(), preset);
    }
  }, [served, name, preset, onLive]);

  return (
    <section aria-labelledby={`${base}-title`} className="mt-2 space-y-3 rounded-md border border-border p-4">
      <h3 id={`${base}-title`} className="text-sm font-semibold">
        {t("apps.generate.newEndpoint.title")}
      </h3>
      {rejected ? null : <p className="text-sm text-fg-muted">{t("apps.generate.newEndpoint.hint")}</p>}
      {renderError && <Alert tone="danger">{renderError}</Alert>}
      {change ? (
        <>
          <ChangeNotice change={phase ? { ...change, status: { ...change.status, phase } } : change} project={project} />
          <p role="status" className={rejected ? "text-sm font-medium text-danger" : "text-sm"}>
            {rejected
              ? t("agentRun.follow.rejected")
              : t("apps.generate.newEndpoint.waiting", { name: name.trim(), lane: lane ?? "" })}
          </p>
          <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
            {t("apps.generate.newEndpoint.close")}
          </Button>
        </>
      ) : (
        // Inside the builder's own form, which a nested `<form>` would break: Enter in the name
        // proposes the endpoint here rather than starting the app.
        <div className="space-y-3">
          <Field
            id={ids.name}
            label={t("apps.generate.newEndpoint.name")}
            help={t("apps.generate.newEndpoint.nameHint")}
            required
            errors={touched && missing.includes("name") ? [t("apps.generate.newEndpoint.nameInvalid")] : undefined}
          >
            <Input
              id={ids.name}
              value={name}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => {
                setName(event.target.value);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  propose();
                }
              }}
            />
          </Field>
          <Field
            id={ids.space}
            label={t("apps.generate.newEndpoint.space")}
            required
            errors={touched && missing.includes("space") ? [t("apps.generate.newEndpoint.spaceMissing")] : undefined}
          >
            <ResourceNamePicker
              id={ids.space}
              label={t("apps.generate.newEndpoint.space")}
              labelled
              from={{ project, plural: "spaces" }}
              value={space}
              onChange={(next) => {
                setSpace(next);
                setTypes([]);
              }}
              required
              invalid={touched && missing.includes("space")}
            />
          </Field>
          <RadioGroup<NewEndpointPreset>
            name={ids.preset}
            legend={t("apps.generate.needs.access")}
            description={t("apps.generate.newEndpoint.accessHint")}
            value={preset}
            options={NEW_ENDPOINT_PRESETS.map((value) => ({
              value,
              label: t(`apps.generate.needs.presets.${value}`),
            }))}
            onChange={setPreset}
          />
          {preset !== "read" && (
            <Field
              id={ids.types}
              label={t("apps.generate.newEndpoint.types")}
              help={t("apps.generate.newEndpoint.typesHint")}
              required
              errors={touched && missing.includes("types") ? [t("apps.generate.newEndpoint.typesMissing")] : undefined}
            >
              <TypePicker
                id={ids.types}
                label={t("apps.generate.newEndpoint.types")}
                labelled
                project={project}
                space={space === "" ? undefined : space}
                value={types}
                onChange={setTypes}
                multiple
                disabled={space === ""}
                required
                invalid={touched && missing.includes("types")}
              />
            </Field>
          )}
          <div className="flex flex-wrap gap-2">
            <Button type="button" loading={render.isPending} onClick={propose}>
              {t("apps.generate.newEndpoint.propose")}
            </Button>
            <Button type="button" variant="ghost" onClick={onCancel}>
              {t("apps.generate.newEndpoint.cancel")}
            </Button>
          </div>
        </div>
      )}
    </section>
  );
}
