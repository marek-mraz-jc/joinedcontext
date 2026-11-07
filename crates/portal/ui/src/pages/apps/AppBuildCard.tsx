import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { Alert, Button, buttonClass, Card } from "../../components/ui";
import { endpointFields, PRESETS, slugOf, useAppNeeds } from "./AppGenerator";
import type { Preset } from "./AppGenerator";

/** Who may open the app, as the conversation asks it (T-2721); a run is never public (AP-42). */
const AUDIENCES = ["project", "organization", "private"] as const;
type Audience = (typeof AUDIENCES)[number];

/** What the conversation answered, as its `app-build` event carries it (API/04 §Paths). */
export interface AppBuild {
  endpoints: string[];
  access: Preset;
  visibility: Audience;
  prompt: string;
}

/** The `app-build` event's payload, or `null` when it is not one the card can build from. */
export function appBuildOf(payload: Record<string, unknown>): AppBuild | null {
  const endpoints = Array.isArray(payload.endpoints)
    ? payload.endpoints.filter((name): name is string => typeof name === "string" && name !== "")
    : [];
  const access = PRESETS.find((preset) => preset === payload.access);
  const visibility = AUDIENCES.find((audience) => audience === payload.visibility);
  const prompt = typeof payload.prompt === "string" ? payload.prompt.trim() : "";
  if (endpoints.length === 0 || !access || !visibility || prompt === "") return null;
  return { endpoints, access, visibility, prompt };
}

/**
 * "Build an app" ready in the conversation (T-2721, API/04 §Paths): what the person answered and
 * Build, which starts the run from this browser with the person's own session, so the run is
 * checked against the grants they hold like any run (AP-132, PF-70), and the conversation never
 * starts one itself (AG-11). The data needs are the builder's, from what the endpoints publish;
 * a write the person's grant does not carry falls back to reading, and the card says so.
 */
export function AppBuildCard({
  project,
  build,
  onStarted,
}: {
  project: string;
  build: AppBuild;
  /** Called with the new run, so the conversation follows it. */
  onStarted?: (runId: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [primary, ...extra] = build.endpoints;
  const { endpoints, endpoint, schema, chosenPreset, needs } = useAppNeeds(project, primary, extra, build.access);
  const name = slugOf(build.prompt, primary);
  const [startedAs, setStartedAs] = useState<string | null>(null);

  const start = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project } },
          body: {
            appName: name,
            appClass: "ui",
            ...endpointFields(primary, extra),
            prompt: build.prompt,
            visibility: build.visibility,
            dataNeeds: needs,
          },
        }),
      ),
    onSuccess: (result) => {
      const created = result as unknown as { id?: string; appName?: string };
      const appName = created.appName || name;
      void queryClient.invalidateQueries({ queryKey: ["projects", project, "agent-runs"] });
      setStartedAs(appName);
      if (typeof created.id === "string") {
        onStarted?.(created.id);
        void navigate({ to: "/projects/$project/$plural/$name", params: { project, plural: "apps", name: appName } });
      }
    },
  });

  const unknown = !endpoints.isPending && !endpoint;
  const reading = endpoints.isPending || (endpoint !== undefined && schema.isPending);
  const blocked = unknown
    ? t("apps.buildCard.noEndpoint", { name: primary })
    : schema.isError
      ? t("apps.buildCard.unavailable", { name: primary })
      : !reading && needs.length === 0
        ? t("apps.buildCard.nothing", { name: primary })
        : null;
  const failure =
    start.error instanceof ApiError
      ? start.error.status === 409
        ? t("apps.buildCard.conflict", { name })
        : (start.error.problem?.detail ?? start.error.message)
      : start.error
        ? t("app.error.generic")
        : null;
  const titleId = `app-build-${name}`;

  return (
    <Card as="section" padding="sm" aria-labelledby={titleId} className="space-y-2">
      <h3 id={titleId} className="text-sm font-semibold">
        {t("apps.buildCard.title", { name })}
      </h3>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
        <dt className="text-fg-muted">{t("apps.buildCard.reads")}</dt>
        <dd>{build.endpoints.join(", ")}</dd>
        <dt className="text-fg-muted">{t("apps.buildCard.may")}</dt>
        <dd>{t(`apps.buildCard.access.${chosenPreset}`)}</dd>
        <dt className="text-fg-muted">{t("apps.buildCard.opens")}</dt>
        <dd>{t(`apps.buildCard.audience.${build.visibility}`)}</dd>
        <dt className="text-fg-muted">{t("apps.buildCard.does")}</dt>
        <dd className="whitespace-pre-wrap break-words">{build.prompt}</dd>
      </dl>
      {!reading && !blocked && chosenPreset !== build.access ? (
        <p className="text-caption text-fg-muted">{t("apps.buildCard.readOnly", { name: primary })}</p>
      ) : null}
      {blocked ? <Alert tone="danger">{blocked}</Alert> : null}
      {failure ? <Alert tone="danger">{failure}</Alert> : null}
      {startedAs ? (
        <p role="status" className="text-sm">
          {t("apps.buildCard.started", { name: startedAs })}{" "}
          <Link
            to="/projects/$project/$plural/$name"
            params={{ project, plural: "apps", name: startedAs }}
            className={buttonClass("secondary", "xs")}
          >
            {t("apps.buildCard.open")}
          </Link>
        </p>
      ) : (
        <Button
          variant="primary"
          size="sm"
          disabled={reading || blocked !== null || start.isPending}
          disabledReason={reading ? t("apps.buildCard.reading") : (blocked ?? undefined)}
          onClick={() => {
            start.mutate();
          }}
        >
          {start.isPending ? t("apps.buildCard.building") : t("apps.buildCard.build")}
        </Button>
      )}
    </Card>
  );
}
