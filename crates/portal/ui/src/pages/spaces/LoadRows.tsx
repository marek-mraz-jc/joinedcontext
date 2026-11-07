/**
 * The second half of a file's way into a Context Space, the Upload page's and Complete this
 * space's (DM-81, T-6445): follows the proposed Change, checks every row against the model, and
 * once the Change is live upserts the rows through the gateway with the person's own session.
 */
import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { download, originTransport } from "@joinedcontext/sdk";
import { api, queryKeys, unwrap } from "../../api/client";
import { useOrgDomain } from "../../api/projects";
import { asManifests, refName } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { ChangeDecide, ChangeNotice } from "../../components/ChangeNotice";
import { spaceSegment } from "../../components/endpoints/policyBinding";
import { Alert, Button, buttonClass, Card, CodeBlock } from "../../components/ui";
import { createInBatches, importSlotsOf, rejectedCsv, rowIds, suggestMapping, toEntities } from "./importRows";
import type { Rejected, Table } from "./importRows";

/** How many entities the preview shows as NGSI-LD before they are written. */
const PREVIEW = 10;

type Checked = ReturnType<typeof toEntities>;
export type Written = { written: number; rejected: Rejected[]; denied?: string };

const phaseOf = (change: Change | null | undefined) => change?.status.phase;

/** The slug of a live Endpoint of the space. */
function endpointOf(items: { spec?: unknown; status?: { phase?: string } | null }[] | undefined, space: string): string | undefined {
  const spec = (one: { spec?: unknown }) => one.spec as { contextSpaceRef?: unknown; slug?: unknown };
  const live = items?.find((one) => one.status?.phase === "Live" && refName(spec(one).contextSpaceRef) === space);
  const slug = live && spec(live).slug;
  return typeof slug === "string" ? slug : undefined;
}

/** The Change as it moves, and what a write needs once it is live: the Endpoint and the space's segment. */
export function useLoadTarget(project: string, space: string, proposed: Change | null) {
  const changeId = proposed?.metadata.name ?? "";
  const followed = useQuery({
    queryKey: queryKeys.change(project, changeId),
    enabled: changeId !== "",
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/changes/{id}", { params: { path: { project, id: changeId } } })),
    refetchInterval: (query) => (phaseOf(query.state.data) === "PendingApproval" || phaseOf(query.state.data) === undefined ? 5_000 : false),
  });
  const change = followed.data ?? proposed;
  // The rows go in once the person's write grant is live: a merged Change is only in Git, and a
  // write before the reconciler took it is refused row by row (T-5109). The op names the grant.
  const merged = phaseOf(change) === "Deploying" || phaseOf(change) === "Merged" || phaseOf(change) === "Applied";
  const grantName = `${space}-upload`;
  const grant = useQuery({
    queryKey: queryKeys.resource(project, "policies", grantName),
    enabled: merged,
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/{plural}/{name}", { params: { path: { project, plural: "policies", name: grantName } } })),
    refetchInterval: (query) => (query.state.data?.status?.phase === "Live" ? false : 3_000),
  });
  // Writes go through an Endpoint of the space and its Policy: the op drafts one when there is none.
  const endpoints = useQuery({
    queryKey: [...queryKeys.resource(project, "spaces", space), "endpoints"],
    enabled: merged || change === null,
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "endpoints" } } })),
    refetchInterval: (query) => (endpointOf(query.state.data?.items, space) ? false : 3_000),
  });
  const endpoint = endpointOf(endpoints.data?.items, space);
  const live = endpoint !== undefined && (change === null || grant.data?.status?.phase === "Live");
  // The ids carry the space's segment, which is not its name (PF-84).
  const held = useQuery({
    queryKey: queryKeys.resource(project, "spaces", space),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/{plural}/{name}", { params: { path: { project, plural: "spaces", name: space } } })),
    enabled: live,
    retry: false,
  });
  const segment = spaceSegment(project, space, held.data ? asManifests([held.data]) : []);
  return { change, merged, live, endpoint, segment };
}

/** How many rows passed the model, and the first of them as they will be written. */
export function RowsCheck({ checked }: { checked: Checked | null }): JSX.Element {
  const { t } = useTranslation();
  if (!checked) return <Alert tone="warning">{t("spaces.upload.noSlots")}</Alert>;
  const count = Math.min(PREVIEW, checked.entities.length);
  return (
    <>
      <p role="status" className="text-body">
        {t("spaces.import.checked", { valid: checked.entities.length, refused: checked.rejected.length })}
      </p>
      {count > 0 ? (
        <details>
          <summary className="focus-ring cursor-pointer rounded-md text-body font-medium text-fg">{t("spaces.upload.preview", { count })}</summary>
          <CodeBlock label={t("spaces.upload.preview", { count })} className="max-h-80 rounded-md bg-surface-muted p-3 font-mono text-caption" data-testid="upload-preview">
            {JSON.stringify(checked.entities.slice(0, PREVIEW).map((each) => each.entity), null, 2)}
          </CodeBlock>
        </details>
      ) : null}
    </>
  );
}

/** What the write did: a refusal, the count, then the file of refused rows or the way into the space. */
export function LoadResult({ done, project, space, type }: { done: Written; project: string; space: string; type: string }): JSX.Element {
  const { t } = useTranslation();
  const first = done.rejected[0];
  return (
    <>
      {done.denied !== undefined ? (
        <Alert role="alert" tone="danger">
          {t("spaces.import.denied", { type, space, reason: done.denied })}
        </Alert>
      ) : null}
      <Alert role="status" tone={done.rejected.length === 0 ? "success" : done.written > 0 ? "warning" : "danger"}>
        {t("spaces.upload.done", { written: done.written, refused: done.rejected.length })}
        {first ? ` ${t("spaces.upload.firstRefusal", { row: first.row, reason: first.reason })}` : null}
      </Alert>
      {first ? (
        <Button
          variant="secondary"
          className="w-fit"
          onClick={() => download(new Blob([rejectedCsv(done.rejected)], { type: "text/csv" }), `${type}-refused.csv`)}
        >
          {t("spaces.import.downloadRefused")}
        </Button>
      ) : (
        <Link to="/projects/$project/$plural/$name" params={{ project, plural: "spaces", name: space }} className={buttonClass("primary", "sm") + " w-fit"}>
          {t("spaces.upload.openSpace")}
        </Link>
      )}
    </>
  );
}

export function LoadRows({ project, space, type, source, table, change: proposed }: {
  project: string;
  space: string;
  type: string;
  /** The LinkML of the proposed model, whose slots check each row. */
  source: string | undefined;
  table: Table;
  change: Change | null;
}): JSX.Element {
  const { t } = useTranslation();
  const orgDomain = useOrgDomain(project);
  const [progress, setProgress] = useState<{ sent: number; total: number } | null>(null);
  const [done, setDone] = useState<Written | null>(null);

  const { change, merged, live, endpoint, segment } = useLoadTarget(project, space, proposed);

  const slots = useMemo(() => importSlotsOf(source, type), [source, type]);
  const checked = useMemo(() => {
    if (slots.length === 0) return null;
    return toEntities(table, suggestMapping(table.columns, slots), slots, { type, orgDomain, space: segment }, rowIds(table));
  }, [table, slots, type, orgDomain, segment]);

  const write = async () => {
    if (!checked || !endpoint) return;
    const total = checked.entities.length;
    setProgress({ sent: 0, total });
    // Upsert: the same file uploaded again writes the same entities, not a second copy.
    const answer = await createInBatches(originTransport(), segment, checked.entities, (sent) => setProgress({ sent, total }), "upsert", undefined, `/api/endpoint/${encodeURIComponent(endpoint)}`);
    setProgress(null);
    // A 401 or 403 stops the write: the person reads who can change it, not "none refused" (JC1-41).
    setDone({ written: answer.created, rejected: [...checked.rejected, ...answer.rejected].sort((a, b) => a.row - b.row), denied: answer.denied });
  };

  const whole = done !== null && done.written === checked?.entities.length && done.rejected.length === 0;
  const state = live ? "spaces.upload.liveNow" : merged ? "spaces.upload.goingLive" : "spaces.upload.afterApproval";
  return (
    <Card className="flex flex-col gap-4">
      {change ? <ChangeNotice change={change} project={project} plain /> : null}
      {change ? <ChangeDecide id={change.metadata.name} project={project} /> : null}
      <RowsCheck checked={checked} />
      <p className="text-body">{t(state, { space, type })}</p>
      {progress ? (
        <p role="status" className="text-body">
          {t("spaces.import.writing", progress)}
        </p>
      ) : whole ? null : (
        <Button className="w-fit" onClick={() => void write()} disabled={!live || !checked || checked.entities.length === 0} disabledReason={t(live ? "spaces.import.nothingToCreate" : "spaces.upload.notLive")}>
          {t("spaces.upload.write", { count: checked?.entities.length ?? 0 })}
        </Button>
      )}
      {done ? <LoadResult done={done} project={project} space={space} type={type} /> : null}
    </Card>
  );
}
