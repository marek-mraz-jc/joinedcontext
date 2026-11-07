import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, JSX, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import type { Change } from "../../api/manifest";
import { isChange } from "../../api/manifest";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Combobox } from "../../components/pickers/Combobox";
import { Alert, Badge, Button, Field, Input, RadioGroup, Textarea } from "../../components/ui";
import type { AccessAsk, FindingAccess } from "./WorkbenchTarget";

/**
 * Access for a pipeline to another space (API/01 §47, PL-90…PL-93, T-4863). The pipeline's people
 * ask the owner of an Endpoint for a read or a write, naming every type and attribute; the request
 * grants nothing. The owners of that space approve, decline or later revoke it on the Endpoint's
 * page; approving proposes the grant as a Change of their own project, through its lane.
 */

export interface AccessRequestView {
  id: string;
  /** The pipeline that asks, or `null` when a person asks for themself (T-5615). */
  pipeline: { project: string; name: string } | null;
  person?: string | null;
  account: string;
  endpoint: { project: string; name: string };
  /** Only on the owners' side: the pipeline's side never learns whether a typed name exists. */
  space?: string;
  operations: Operation;
  types: { type: string; attrs: string[] }[];
  until: string | null;
  reason: string;
  state: "pending" | "approved" | "declined" | "revoked" | "ended";
  requestedBy: string;
  requestedAt: string;
  decidedBy: string | null;
  decidedAt: string | null;
  decisionReason: string | null;
  change: string | null;
}

const KEY = "access-requests";

/** A typed `project/endpoint`, both DNS-1123 labels as the server checks them. */
const TYPED_ENDPOINT = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?\/[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

type Operation = components["schemas"]["Operations"];
type NewRequest = components["schemas"]["NewRequest"];

/** The list routes answer `{ items }` of the view above (API/01 §47). */
async function listed(answer: Promise<unknown>): Promise<{ items: AccessRequestView[] }> {
  const body: unknown = await answer;
  return body as { items: AccessRequestView[] };
}

/** One Endpoint the request may name, with the types and attributes its space's model serves. */
export type RequestableEndpoint = components["schemas"]["Requestable"];

/** A grant the record check found missing, as the request form opens with it (PL-90, T-5369). */
export interface AccessPrefill {
  /** `project/endpoint`. */
  endpoint: string;
  operations: Operation;
  type?: string;
}

/**
 * The Endpoints the request form offers the pipeline, by the rule the request door applies
 * again (T-4946); one query per pipeline, shared by the form and the editor's findings.
 */
export function useRequestable(project: string, name: string | undefined, enabled: boolean) {
  return useQuery({
    queryKey: [KEY, project, name, "endpoints"],
    enabled: enabled && Boolean(name),
    retry: false,
    queryFn: async () => {
      const body: unknown = await unwrap(
        await api.GET("/api/v1/projects/{project}/pipelines/{name}/access-requests/endpoints", {
          params: { path: { project, name: name ?? "" } },
        }),
      );
      return body as { items: RequestableEndpoint[] };
    },
  });
}

/** The request form's frame: a `<form>`, or inside another form a `role="form"` region sent by its button. */
function Shell({ nested, onSubmit, labelledBy, children }: { nested?: boolean; onSubmit: (event: FormEvent) => void; labelledBy: string; children: ReactNode }): JSX.Element {
  const className = "flex flex-col gap-3 rounded-md border border-border p-3";
  return nested ? (
    <div role="form" aria-labelledby={labelledBy} className={className}>
      {children}
    </div>
  ) : (
    <form onSubmit={onSubmit} noValidate aria-labelledby={labelledBy} className={className}>
      {children}
    </form>
  );
}

/** The form's first state: closed and empty, or open with what the editor already knows. */
function initialRequest(prefill: AccessPrefill | undefined) {
  return {
    open: prefill !== undefined,
    endpoint: prefill?.endpoint ?? "",
    operations: prefill?.operations ?? ("read" as const),
    type: prefill?.type ?? "",
    lead: prefill ? "pipelines.access.prefilled" : "pipelines.access.lead",
  };
}

/** Send and Cancel; nested, Send is a plain button so it never submits the editor's form. */
function FormActions({ nested, pending, onSend, onCancel }: { nested?: boolean; pending: boolean; onSend: () => void; onCancel: () => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-wrap gap-2">
      <Button type={nested ? "button" : "submit"} disabled={pending} disabledReason={t("app.busyReason")} onClick={nested ? onSend : undefined}>
        {t("pipelines.access.send")}
      </Button>
      <Button type="button" variant="secondary" onClick={onCancel}>
        {t("pipelines.access.cancel")}
      </Button>
    </div>
  );
}

/** The request body from the form, or the first thing the person still has to fill in. */
export function requestBody(form: {
  project: string;
  endpoint: string;
  operations: Operation;
  rows: { type: string; attrs: string[] }[];
  until: string;
  reason: string;
}): { body: NewRequest } | { missing: "endpoint" | "types" | "reason" } {
  if (!form.project.trim() || !form.endpoint.trim()) return { missing: "endpoint" };
  const types = form.rows
    .map((row) => ({ type: row.type.trim(), attrs: row.attrs }))
    .filter((row) => row.type !== "" || row.attrs.length > 0);
  if (types.length === 0 || types.some((row) => row.type === "" || row.attrs.length === 0)) return { missing: "types" };
  if (!form.reason.trim()) return { missing: "reason" };
  return {
    body: {
      endpoint: { project: form.project.trim(), name: form.endpoint.trim() },
      operations: form.operations,
      types,
      ...(form.until ? { until: new Date(`${form.until}T23:59:59`).toISOString() } : {}),
      reason: form.reason.trim(),
    },
  };
}

function problem(err: unknown, fallback: string): string {
  return err instanceof ApiError ? (err.problem?.detail ?? err.message) : err instanceof Error ? err.message : fallback;
}

export function StateBadge({ state }: { state: AccessRequestView["state"] }): JSX.Element {
  const { t } = useTranslation();
  const tone = state === "approved" ? "success" : state === "pending" ? "warning" : "neutral";
  return <Badge tone={tone}>{t(`pipelines.access.state.${state}`)}</Badge>;
}

export function Grant({ request }: { request: AccessRequestView }): JSX.Element {
  const { t, i18n } = useTranslation();
  const day = new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium" });
  return (
    <div className="flex flex-col gap-1 text-caption text-fg-muted">
      <span>
        {t(`pipelines.access.operations.${request.operations}`)}
        {" · "}
        {request.types
          .map((row) => `${row.type} (${row.attrs.length > 0 ? row.attrs.join(", ") : t("pipelines.access.allAttrs")})`)
          .join("; ")}
      </span>
      <span>
        {request.until ? t("pipelines.access.until", { date: day.format(new Date(request.until)) }) : t("pipelines.access.noEnd")}
      </span>
      {request.decisionReason ? (
        <span>{t("pipelines.access.decided", { who: request.decidedBy ?? "", reason: request.decisionReason })}</span>
      ) : null}
    </div>
  );
}

/**
 * The pipeline's requests and the form that files a new one (PL-90); `prefill` opens the form
 * with the Endpoint, operation and type of a grant the record check found missing (T-5369).
 */
export function PipelineAccessRequests({
  project,
  name,
  prefill,
  nested,
}: {
  project: string;
  name: string;
  prefill?: AccessPrefill;
  /** Inside another form (the pipeline editor): no `<form>` of its own, which HTML forbids there. */
  nested?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const [initial] = useState(() => initialRequest(prefill));
  const [open, setOpen] = useState(initial.open);
  const [chosen, setChosen] = useState(initial.endpoint);
  const [operations, setOperations] = useState<Operation>(initial.operations);
  const [rows, setRows] = useState<{ type: string; attrs: string[] }[]>([{ type: initial.type, attrs: [] }]);
  const [until, setUntil] = useState("");
  const [reason, setReason] = useState("");
  const [missing, setMissing] = useState<string | null>(null);

  const list = useQuery({
    queryKey: [KEY, project, name],
    retry: false,
    queryFn: async () =>
      listed(
        unwrap(
          await api.GET("/api/v1/projects/{project}/pipelines/{name}/access-requests", {
            params: { path: { project, name } },
          }),
        ),
      ),
  });
  const file = useMutation({
    mutationFn: async (body: NewRequest) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/pipelines/{name}/access-requests", {
          params: { path: { project, name } },
          body,
        }),
      ),
    onSuccess: () => {
      setOpen(false);
      setRows([{ type: "", attrs: [] }]);
      setReason("");
      void queryClient.invalidateQueries({ queryKey: [KEY, project, name] });
    },
  });

  // The choices come from the server, by the rule the request door applies again (T-4946).
  const offered = useRequestable(project, name, open);
  const endpoints = offered.data?.items ?? [];
  const target = endpoints.find((one) => `${one.project}/${one.name}` === chosen);
  // An Endpoint the list does not offer is asked for by its typed name; its owners check the
  // types and attributes then, so those are typed too (T-4946).
  const typed = !target && TYPED_ENDPOINT.test(chosen);
  const [owner, endpoint] = target ? [target.project, target.name] : typed ? chosen.split("/") : ["", ""];
  const typesOf = (type: string) => target?.types.find((one) => one.type === type)?.attrs ?? [];
  const failedList = offered.isError ? { reason: problem(offered.error, t("app.error.generic")), retry: () => void offered.refetch() } : undefined;

  const submit = (event?: FormEvent) => {
    event?.preventDefault();
    const built = requestBody({ project: owner, endpoint, operations, rows, until, reason });
    if ("missing" in built) {
      setMissing(t(`pipelines.access.missing.${built.missing}`));
      return;
    }
    setMissing(null);
    file.mutate(built.body);
  };
  const items = list.data?.items ?? [];

  return (
    <div className="flex flex-col gap-3" data-testid="pipeline-access">
      {list.isError ? (
        <Alert tone="danger" role="alert">
          {problem(list.error, t("app.error.generic"))}
        </Alert>
      ) : items.length === 0 ? (
        <p className="text-caption text-fg-muted">{t("pipelines.access.none")}</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {items.map((request) => (
            <li key={request.id} className="flex flex-col gap-1 rounded-md border border-border p-2">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-body">
                  {request.endpoint.project}/{request.endpoint.name}
                </span>
                <StateBadge state={request.state} />
              </div>
              <Grant request={request} />
            </li>
          ))}
        </ul>
      )}
      {open ? (
        <Shell nested={nested} onSubmit={submit} labelledBy={`${ids}-title`}>
          <h3 id={`${ids}-title`} className="text-body font-semibold text-fg">
            {t("pipelines.access.request")}
          </h3>
          <p className="text-caption text-fg-muted">{t(initial.lead)}</p>
          <Field
            id={`${ids}-endpoint`}
            label={t("pipelines.access.endpoint")}
            help={t("pipelines.access.endpointHelp")}
            errors={chosen && !target && !typed ? [t("pipelines.access.endpointTypedBad", { name: chosen })] : undefined}
            required
          >
            <Combobox
              id={`${ids}-endpoint`}
              label={t("pipelines.access.endpoint")}
              labelled
              value={chosen ? [chosen] : []}
              options={endpoints.map((one) => ({
                value: `${one.project}/${one.name}`,
                label: one.name,
                detail: t("pipelines.access.endpointSpace", { space: one.space }),
                group: t("pipelines.access.endpointProject", { project: one.project }),
              }))}
              loading={offered.isLoading}
              failed={failedList}
              empty={t("pipelines.access.noEndpoints")}
              onChange={(values) => {
                setChosen(values[0] ?? "");
                setRows([{ type: "", attrs: [] }]);
              }}
              create={{
                label: (text) => t("pipelines.access.endpointTyped", { name: text }),
                onCreate: (text) => {
                  setChosen(text.trim().toLowerCase());
                  setRows([{ type: "", attrs: [] }]);
                },
              }}
            />
          </Field>
          <RadioGroup
            name={`${ids}-ops`}
            legend={t("pipelines.access.operationsLegend")}
            value={operations}
            layout="row"
            onChange={setOperations}
            options={[
              { value: "read", label: t("pipelines.access.operations.read") },
              { value: "write", label: t("pipelines.access.operations.write") },
              { value: "history", label: t("pipelines.access.operations.history") },
            ]}
          />
          <fieldset className="flex flex-col gap-2">
            <legend className="text-body font-medium text-fg">{t("pipelines.access.types")}</legend>
            <p className="text-caption text-fg-muted">{t("pipelines.access.typesHelp")}</p>
            {rows.map((row, at) => (
              <div key={at} className="flex flex-wrap items-end gap-2">
                <Field
                  id={`${ids}-type-${at}`}
                  label={t("pipelines.access.type")}
                  help={
                    typed
                      ? t("pipelines.access.typedTypeHelp")
                      : target && target.types.length === 0
                        ? t("pipelines.access.noTypes")
                        : t("pipelines.access.typeHelp")
                  }
                >
                  <Combobox
                    id={`${ids}-type-${at}`}
                    label={t("pipelines.access.type")}
                    labelled
                    disabled={!target && !typed}
                    value={row.type ? [row.type] : []}
                    options={(target?.types ?? [])
                      .filter((one) => one.type === row.type || !rows.some((other) => other.type === one.type))
                      .map((one) => ({ value: one.type, label: one.type, group: target?.space ?? "" }))}
                    empty={typed ? t("pipelines.access.typedTypeHelp") : target ? t("pipelines.access.noTypes") : t("pipelines.access.endpointFirst")}
                    onChange={(values) => setRows(rows.map((r, i) => (i === at ? { type: values[0] ?? "", attrs: [] } : r)))}
                    create={
                      typed
                        ? {
                            label: (text) => t("picker.type.create", { name: text }),
                            onCreate: (text) => setRows(rows.map((r, i) => (i === at ? { type: text.trim(), attrs: [] } : r))),
                          }
                        : undefined
                    }
                  />
                </Field>
                <Field id={`${ids}-attrs-${at}`} label={t("pipelines.access.attrs")} help={t("pipelines.access.attrsHelp")}>
                  <Combobox
                    id={`${ids}-attrs-${at}`}
                    label={t("pipelines.access.attrs")}
                    labelled
                    multiple
                    disabled={!row.type}
                    value={row.attrs}
                    options={typesOf(row.type).map((attr) => ({ value: attr, label: attr, group: row.type }))}
                    empty={row.type ? t("pipelines.access.noAttrs", { type: row.type }) : t("pipelines.access.typeFirst")}
                    onChange={(values) => setRows(rows.map((r, i) => (i === at ? { ...r, attrs: values } : r)))}
                    create={
                      typed
                        ? {
                            label: (text) => t("picker.createNamed", { name: text }),
                            onCreate: (text) =>
                              setRows(rows.map((r, i) => (i === at && !r.attrs.includes(text.trim()) ? { ...r, attrs: [...r.attrs, text.trim()] } : r))),
                          }
                        : undefined
                    }
                  />
                </Field>
                {rows.length > 1 ? (
                  <Button
                    type="button"
                    size="sm"
                    variant="secondary"
                    aria-label={t("pipelines.access.removeTypeOf", { n: at + 1 })}
                    onClick={() => setRows(rows.filter((_, i) => i !== at))}
                  >
                    {t("pipelines.access.removeType")}
                  </Button>
                ) : null}
              </div>
            ))}
            <div>
              <Button
                type="button"
                size="sm"
                variant="secondary"
                disabled={rows.length >= 20}
                disabledReason={t("pipelines.access.tooManyTypes")}
                onClick={() => setRows([...rows, { type: "", attrs: [] }])}
              >
                {t("pipelines.access.addType")}
              </Button>
            </div>
          </fieldset>
          <Field id={`${ids}-until`} label={t("pipelines.access.untilLabel")} help={t("pipelines.access.untilHelp")}>
            <Input id={`${ids}-until`} type="date" value={until} onChange={(e) => setUntil(e.target.value)} />
          </Field>
          <Field id={`${ids}-reason`} label={t("pipelines.access.reason")} required>
            <Textarea id={`${ids}-reason`} value={reason} maxLength={500} onChange={(e) => setReason(e.target.value)} />
          </Field>
          {missing ? (
            <Alert tone="danger" role="alert">
              {missing}
            </Alert>
          ) : null}
          {file.isError ? (
            <Alert tone="danger" role="alert">
              {problem(file.error, t("app.error.generic"))}
            </Alert>
          ) : null}
          <FormActions nested={nested} pending={file.isPending} onSend={() => submit()} onCancel={() => setOpen(false)} />
        </Shell>
      ) : (
        <div>
          {file.isSuccess ? (
            <Alert tone="success" role="status">
              {t("pipelines.access.sent")}
            </Alert>
          ) : null}
          <Button variant="secondary" size="sm" onClick={() => setOpen(true)}>
            {t("pipelines.access.request")}
          </Button>
        </div>
      )}
    </div>
  );
}

/**
 * The record check's missing grants as "No access to <space>: Request access" (T-5369): the ask
 * each finding line renders, and the request form it opens, prefilled. The space comes only from
 * the server's list for this person (T-4946); an Endpoint it does not offer reads `project/name`.
 * A pipeline not yet proposed has no account to grant, so the button says to propose it first.
 */
export function useAccessAsk(project: string, pipeline: string | undefined, wanted: boolean): { ask: AccessAsk; form: JSX.Element | null } {
  const { t } = useTranslation();
  const [asking, setAsking] = useState<FindingAccess | null>(null);
  const offered = useRequestable(project, pipeline, wanted);
  const section = useRef<HTMLElement>(null);
  useEffect(() => {
    if (asking) section.current?.scrollIntoView?.({ block: "nearest" });
  }, [asking]);
  const ask: AccessAsk = {
    where: (access) =>
      offered.data?.items.find((one) => one.project === access.project && one.name === access.name)?.space ?? `${access.project}/${access.name}`,
    open: setAsking,
    ...(pipeline ? {} : { disabledReason: t("pipelines.access.proposeFirst") }),
  };
  const form =
    asking && pipeline ? (
      <section ref={section} aria-label={t("pipelines.access.section")} data-testid="workbench-access">
        <PipelineAccessRequests
          key={`${asking.operations}:${asking.project}/${asking.name}:${asking.type ?? ""}`}
          project={project}
          name={pipeline}
          nested
          prefill={{ endpoint: `${asking.project}/${asking.name}`, operations: asking.operations, ...(asking.type ? { type: asking.type } : {}) }}
        />
      </section>
    ) : null;
  return { ask, form };
}

/** The requests into one space of this project, for its owners (PL-91, PL-93). */
export function AccessRequestInbox({ project, space }: { project: string; space: string }): JSX.Element | null {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [change, setChange] = useState<Change | null>(null);
  const [reasons, setReasons] = useState<Record<string, string>>({});
  const list = useQuery({
    queryKey: [KEY, "in", project],
    retry: false,
    queryFn: async () =>
      listed(unwrap(await api.GET("/api/v1/projects/{project}/access-requests", { params: { path: { project } } }))),
  });
  const act = useMutation({
    mutationFn: async ({ id, verb }: { id: string; verb: "approve" | "decline" | "revoke" }) => {
      const path = { params: { path: { project, id } } };
      const reason = { reason: reasons[id] ?? "" };
      if (verb === "approve") return unwrap(await api.POST("/api/v1/projects/{project}/access-requests/{id}/approve", path));
      if (verb === "decline")
        return unwrap(await api.POST("/api/v1/projects/{project}/access-requests/{id}/decline", { ...path, body: reason }));
      return unwrap(await api.POST("/api/v1/projects/{project}/access-requests/{id}/revoke", { ...path, body: reason }));
    },
    onSuccess: (result) => {
      if (isChange(result)) setChange(result);
      void queryClient.invalidateQueries({ queryKey: [KEY, "in", project] });
    },
  });
  const items = (list.data?.items ?? []).filter((request) => request.space === space);
  if (list.isError || (list.isSuccess && items.length === 0)) {
    return list.isError ? null : <p className="text-caption text-fg-muted">{t("pipelines.access.inboxEmpty")}</p>;
  }
  return (
    <div className="flex flex-col gap-2" data-testid="access-inbox">
      {act.isError ? (
        <Alert tone="danger" role="alert">
          {problem(act.error, t("app.error.generic"))}
        </Alert>
      ) : null}
      {change ? <ChangeNotice change={change} project={project} /> : null}
      <ul className="flex flex-col gap-2">
        {items.map((request) => {
          const reasonId = `access-reason-${request.id}`;
          const decidable = request.state === "pending" || request.state === "approved";
          return (
            <li key={request.id} className="flex flex-col gap-2 rounded-md border border-border p-2">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-body">{request.account}</span>
                <span className="text-caption text-fg-muted">
                  {t("pipelines.access.on", { endpoint: request.endpoint.name, who: request.requestedBy })}
                </span>
                <StateBadge state={request.state} />
              </div>
              <Grant request={request} />
              <p className="text-caption text-fg">{request.reason}</p>
              {decidable ? (
                <div className="flex flex-wrap items-end gap-2">
                  <Field id={reasonId} label={t("pipelines.access.decisionReason")}>
                    <Input
                      id={reasonId}
                      value={reasons[request.id] ?? ""}
                      maxLength={500}
                      onChange={(e) => setReasons({ ...reasons, [request.id]: e.target.value })}
                    />
                  </Field>
                  {request.state === "pending" ? (
                    <>
                      <Button
                        size="sm"
                        disabled={act.isPending}
                        disabledReason={t("app.busyReason")}
                        aria-label={t("pipelines.access.approveOf", { account: request.account })}
                        onClick={() => act.mutate({ id: request.id, verb: "approve" })}
                      >
                        {t("pipelines.access.approve")}
                      </Button>
                      <Button
                        size="sm"
                        variant="secondary"
                        disabled={act.isPending || !(reasons[request.id] ?? "").trim()}
                        disabledReason={t("pipelines.access.reasonFirst")}
                        aria-label={t("pipelines.access.declineOf", { account: request.account })}
                        onClick={() => act.mutate({ id: request.id, verb: "decline" })}
                      >
                        {t("pipelines.access.decline")}
                      </Button>
                    </>
                  ) : (
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={act.isPending || !(reasons[request.id] ?? "").trim()}
                      disabledReason={t("pipelines.access.reasonFirst")}
                      aria-label={t("pipelines.access.revokeOf", { account: request.account })}
                      onClick={() => act.mutate({ id: request.id, verb: "revoke" })}
                    >
                      {t("pipelines.access.revoke")}
                    </Button>
                  )}
                </div>
              ) : null}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
