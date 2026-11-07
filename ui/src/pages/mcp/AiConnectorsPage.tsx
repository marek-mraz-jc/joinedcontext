import { clsx } from "clsx";
import { Fragment, useCallback, useState, useSyncExternalStore } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError, api, unwrap } from "../../api/client";
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  Dialog,
  EmptyState,
  Field,
  Input,
  PageFailed,
  PageHeader,
  PageLoading,
  Select,
} from "../../components/ui";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { CopyValue } from "./McpServerPanels";
import { connectTab, disconnectTab, onPairingChange, pairing } from "../../connectors/pairedTab";
import {
  ceilingAllows,
  clientLabel,
  clientRefusal,
  emptyForm,
  formProblems,
  kindRows,
  lastDay,
  pickKey,
  readsShared,
  refusalHolding,
  refusedSection,
  scopeLines,
  setup,
  toForm,
  toRequest,
  verbsOf,
  type ChangeVerb,
} from "./aiConnectors";
import type { AiConnector, AiConnectorForm, ConnectorOptions, Pick, PickKind } from "./aiConnectors";

const KEY = ["ai-connectors"] as const;
const OPTIONS_KEY = ["ai-connectors", "options"] as const;

function useOptions() {
  return useQuery({ queryKey: OPTIONS_KEY, queryFn: async () => unwrap(await api.GET("/api/v1/connectors/options")) });
}

/**
 * The person's AI connectors (AG-142, API/14, T-5951): each a Portal MCP address an AI client
 * adds, reaching only the projects, spaces, Endpoints and pipelines ticked here, never more than
 * the person may do themself. The page shows no token: the client signs the person in itself.
 */
export function AiConnectorsPage(): JSX.Element {
  const { t } = useTranslation();
  const list = useQuery({ queryKey: KEY, queryFn: async () => unwrap(await api.GET("/api/v1/connectors")) });
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<string | null>(null);
  const items = list.data?.items ?? [];
  const live = items.filter((one) => one.state === "active").length;
  // What was revoked before this visit folds away; a card revoked just now stays to say so.
  const [seenLive, setSeenLive] = useState<ReadonlySet<string>>(new Set());
  const newlyLive = items.filter((one) => one.state !== "revoked" && !seenLive.has(one.id));
  if (newlyLive.length > 0) setSeenLive(new Set([...seenLive, ...newlyLive.map((one) => one.id)]));
  const isFolded = (one: AiConnector) => one.state === "revoked" && !seenLive.has(one.id);
  const folded = items.filter(isFolded);
  const create = (
    <Button disabled={live >= 20} disabledReason={t("aiConnectors.full")} onClick={() => setCreating(true)}>
      {t("aiConnectors.new")}
    </Button>
  );
  return (
    <section aria-label={t("aiConnectors.title")} className="space-y-6">
      <PageHeader title={t("aiConnectors.title")} description={t("aiConnectors.lead")} actions={create} />
      {list.isPending ? <PageLoading label={t("aiConnectors.loading")} /> : null}
      {list.isError ? <PageFailed error={list.error} onRetry={() => void list.refetch()} /> : null}
      {list.isSuccess && items.length === 0 ? (
        <EmptyState title={t("aiConnectors.empty")} description={t("aiConnectors.emptyHint")} icon="endpoints" action={create} />
      ) : null}
      {items
        .filter((one) => !isFolded(one))
        .map((connector) => (
          <AiConnectorCard key={connector.id} connector={connector} justCreated={connector.id === created} />
        ))}
      {folded.length > 0 ? (
        <details>
          <summary className="focus-ring w-fit cursor-pointer rounded-sm text-body font-medium">
            {t("aiConnectors.revokedList", { count: folded.length })}
          </summary>
          <div className="mt-3 space-y-6">
            {folded.map((connector) => (
              <AiConnectorCard key={connector.id} connector={connector} />
            ))}
          </div>
        </details>
      ) : null}
      {creating ? (
        <CreateDialog
          onClose={() => setCreating(false)}
          onCreated={(id) => {
            setCreated(id);
            setCreating(false);
          }}
        />
      ) : null}
    </section>
  );
}

const STATE_TONE = { active: "success", pending: "warning", expired: "warning", revoked: "neutral" } as const;

export function AiConnectorCard({ connector, justCreated }: { connector: AiConnector; justCreated?: boolean }): JSX.Element {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const verbWord = (verb: string) => t(`aiConnectors.verb.${verb}`);
  const [showSetup, setShowSetup] = useState(Boolean(justCreated));
  const [showActivity, setShowActivity] = useState(false);
  const [editing, setEditing] = useState(false);
  const revoke = useMutation({
    mutationFn: async () => unwrap(await api.DELETE("/api/v1/connectors/{id}", { params: { path: { id: connector.id } } })),
    onSuccess: () => {
      setConfirming(false);
      void client.invalidateQueries({ queryKey: KEY });
    },
  });
  const pairedHere = useSyncExternalStore(onPairingChange, () => pairing()?.connector === connector.id, () => false);
  const tab = useMutation({
    mutationFn: async () => (pairedHere ? disconnectTab() : connectTab(connector.id).then(() => undefined)),
  });
  const when = (at: string | null | undefined) => (at ? new Date(at).toLocaleString(i18n.language) : t("aiConnectors.never"));
  const titleId = `ai-connector-${connector.id}`;
  const active = connector.state === "active";
  // Shared datasets, read only (ADR-N-067): no project of the person's for a paired tab to work in.
  const shared = readsShared(connector, useOptions().data);
  const tabOffered = active && !shared;
  return (
    <Card>
      <section aria-labelledby={titleId} className="flex flex-col gap-4">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <h2 id={titleId} className="font-mono text-title font-semibold">
              {connector.name}
            </h2>
            <Badge tone={STATE_TONE[connector.state as keyof typeof STATE_TONE] ?? "neutral"}>{t(`aiConnectors.state.${connector.state}`)}</Badge>
          </div>
          <div className="flex flex-wrap gap-2">
            {active ? (
              <Button variant="secondary" size="sm" aria-expanded={showSetup} onClick={() => setShowSetup(!showSetup)}>
                {t("aiConnectors.setup")}
              </Button>
            ) : null}
            {tabOffered || pairedHere ? (
              <Button
                variant="secondary"
                size="sm"
                aria-pressed={pairedHere}
                aria-label={t(pairedHere ? "pairedTab.disconnectOf" : "pairedTab.connectOf", { name: connector.name })}
                loading={tab.isPending}
                onClick={() => tab.mutate()}
              >
                {t(pairedHere ? "pairedTab.disconnectHere" : "pairedTab.connect")}
              </Button>
            ) : null}
            {connector.state !== "revoked" ? (
              <Button variant="secondary" size="sm" aria-label={t("aiConnectors.editOf", { name: connector.name })} onClick={() => setEditing(true)}>
                {t("aiConnectors.edit")}
              </Button>
            ) : null}
            <Button variant="secondary" size="sm" aria-expanded={showActivity} onClick={() => setShowActivity(!showActivity)}>
              {t("aiConnectors.activity")}
            </Button>
            {connector.state !== "revoked" ? (
              <Button variant="danger" size="sm" aria-label={t("aiConnectors.revokeOf", { name: connector.name })} onClick={() => setConfirming(true)}>
                {t("aiConnectors.revoke")}
              </Button>
            ) : null}
          </div>
        </div>
        {tabOffered && !pairedHere ? <p className="text-caption text-fg-muted">{t("pairedTab.hint")}</p> : null}
        {tab.isError ? (
          <Alert tone="danger" role="alert">
            {tab.error.message}
          </Alert>
        ) : null}
        {confirming ? (
          <Alert tone="warning" role="alert">
            <p>{t("aiConnectors.revokeConfirm", { name: connector.name })}</p>
            {revoke.isError ? <p>{revoke.error.message}</p> : null}
            <div className="mt-2 flex flex-wrap gap-2">
              <Button variant="danger" size="sm" loading={revoke.isPending} onClick={() => revoke.mutate()}>
                {t("aiConnectors.revokeNow")}
              </Button>
              <Button variant="secondary" size="sm" onClick={() => setConfirming(false)}>
                {t("aiConnectors.keep")}
              </Button>
            </div>
          </Alert>
        ) : null}
        {/* Above the facts: right after Create the address is the next thing to reach for. */}
        {showSetup && active ? <SetupPanel name={connector.name} url={connector.url} /> : null}
        <dl className="grid gap-x-6 gap-y-2 text-body sm:grid-cols-2 lg:grid-cols-3">
          <div>
            <dt className="text-caption font-semibold">{t("aiConnectors.field.scope")}</dt>
            <dd>
              <ul aria-label={t("aiConnectors.scopeOf", { name: connector.name })}>
                {scopeLines(connector.scope).map((line) => (
                  <li key={`${line.where}/${line.kind}/${line.name ?? ""}`}>
                    <span className="font-mono">{line.where}</span>
                    {line.name ? ` / ${t(`aiConnectors.kind.${line.kind}`)} ` : ` (${t("aiConnectors.kind.project")})`}
                    {line.name ? <span className="font-mono">{line.name === "*" ? t("aiConnectors.every") : line.name}</span> : null}
                    {" · "}
                    {shared ? t("aiConnectors.organizationRead") : t(`aiConnectors.access.${line.access}`)}
                  </li>
                ))}
              </ul>
            </dd>
          </div>
          <div>
            <dt className="text-caption font-semibold">{t("aiConnectors.field.segments")}</dt>
            <dd>{connector.segments.map((segment) => t(`aiConnectors.segment.${segment}`)).join(", ")}</dd>
            {changeLines(connector, verbWord).length > 0 ? (
              <>
                <dt className="text-caption font-semibold">{t("aiConnectors.field.kinds")}</dt>
                <dd>{changeLines(connector, verbWord).join(" · ")}</dd>
              </>
            ) : null}
          </div>
          <div>
            <dt className="text-caption font-semibold">{t("aiConnectors.field.clients")}</dt>
            <dd>
              {connector.allowedClients.length === 0
                ? t("aiConnectors.anyClient")
                : connector.allowedClients.map((id) => clientLabel(id, t)).join(", ")}
            </dd>
          </div>
          <div>
            <dt className="text-caption font-semibold">{t("aiConnectors.lastCall")}</dt>
            <dd>{when(connector.lastUsedAt)}</dd>
          </div>
          <div>
            <dt className="text-caption font-semibold">{t("aiConnectors.field.expires")}</dt>
            <dd>{when(connector.expiresAt)}</dd>
          </div>
        </dl>
        {showActivity ? <ActivityTable id={connector.id} name={connector.name} /> : null}
        {editing ? <CreateDialog stored={connector} onClose={() => setEditing(false)} onCreated={() => setEditing(false)} /> : null}
      </section>
    </Card>
  );
}

/** What to paste into each AI client; the client then signs the person in (OAuth), no token here. */
export function SetupPanel({ name, url }: { name: string; url: string }): JSX.Element {
  const { t } = useTranslation();
  const snippets = setup(name, url);
  return (
    <div className="flex flex-col gap-3 rounded-md border border-border p-3">
      <h3 className="text-body font-semibold">{t("aiConnectors.setupTitle")}</h3>
      <CopyValue label={t("mcp.address")} value={url} />
      <CopyValue label={t("aiConnectors.client.claudeCode")} value={snippets.claudeCode} code />
      <div className="grid gap-3 sm:grid-cols-2">
        <div>
          <h4 className="text-caption font-semibold">{t("aiConnectors.client.claudeAi")}</h4>
          <p className="text-body text-fg">{t("aiConnectors.steps.claudeAi")}</p>
        </div>
        <div>
          <h4 className="text-caption font-semibold">{t("aiConnectors.client.chatgpt")}</h4>
          <p className="text-body text-fg">{t("aiConnectors.steps.chatgpt")}</p>
        </div>
      </div>
      <details>
        <summary className="cursor-pointer text-caption font-semibold">{t("aiConnectors.client.vscode")}</summary>
        <CopyValue label={t("aiConnectors.client.vscode")} value={snippets.vscode} code />
      </details>
    </div>
  );
}

export function ActivityTable({ id, name }: { id: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const calls = useQuery({
    queryKey: [...KEY, id, "activity"],
    queryFn: async () => unwrap(await api.GET("/api/v1/connectors/{id}/activity", { params: { path: { id } } })),
  });
  if (calls.isPending) return <PageLoading label={t("aiConnectors.activityLoading")} lines={1} />;
  if (calls.isError) return <PageFailed error={calls.error} onRetry={() => void calls.refetch()} />;
  if (calls.data.items.length === 0) return <p className="text-caption text-fg-muted">{t("aiConnectors.noCalls")}</p>;
  // A call has no id of its own: its place in the answer keys the row.
  type Call = (typeof calls.data.items)[number] & { row: number };
  const clientOf = (call: Call) => (call.client ? clientLabel(call.client, t) : "");
  return (
    <ManifestGrid<Call>
      caption={t("aiConnectors.activityOf", { name })}
      storageKey="ai-connector-activity"
      searchLabel={t("grid.searchIn", { list: t("aiConnectors.activityOf", { name }) })}
      rows={calls.data.items.map((call, row) => ({ ...call, row }))}
      rowKey={(call) => String(call.row)}
      query={calls}
      pageSize={20}
      name={{
        id: "at",
        label: t("aiConnectors.call.at"),
        text: (call) => call.at,
        cell: (call) => <time dateTime={call.at}>{new Date(call.at).toLocaleString(i18n.language)}</time>,
      }}
      columns={[
        { id: "tool", label: t("aiConnectors.call.tool"), filter: true, text: (call) => call.tool, cell: (call) => <span className="font-mono">{call.tool}</span> },
        { id: "target", label: t("aiConnectors.call.target"), text: (call) => call.target ?? "", cell: (call) => <span className="font-mono">{call.target ?? "–"}</span> },
        { id: "client", label: t("aiConnectors.call.client"), filter: true, text: clientOf, cell: (call) => clientOf(call) || "–" },
        {
          id: "verdict",
          label: t("aiConnectors.call.verdict"),
          filter: true,
          text: (call) => call.verdict,
          cell: (call) => <Badge tone={call.verdict === "allowed" ? "success" : "danger"}>{call.verdict}</Badge>,
        },
      ]}
      empty={<p className="text-caption text-fg-muted">{t("aiConnectors.noCalls")}</p>}
    />
  );
}

function picked(form: AiConnectorForm, project: string, kind: PickKind, name?: string): Pick | undefined {
  return form.picks.find((pick) => pickKey(pick) === pickKey({ project, kind, name, access: "read" }));
}

/** Above this many spaces, Endpoints and pipelines a project folds them, and the tree gets a search. */
const FOLD_OVER = 12;

/** The tick tree: only what the server lists for this person, so nothing past their own rights. */
export function ScopeTree({
  options,
  form,
  onChange,
}: {
  options: ConnectorOptions;
  form: AiConnectorForm;
  onChange: (picks: Pick[]) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [find, setFind] = useState("");
  if (options.projects.length === 0) {
    return <Alert tone="info">{t("aiConnectors.noProjects")}</Alert>;
  }
  const sizeOf = (project: ConnectorOptions["projects"][number]) => project.spaces.length + project.endpoints.length + project.pipelines.length;
  const searchable = options.projects.reduce((sum, project) => sum + sizeOf(project), 0) > FOLD_OVER;
  const word = find.trim().toLowerCase();
  const toggle = (project: string, kind: PickKind, name: string | undefined, on: boolean) => {
    const key = pickKey({ project, kind, name, access: "read" });
    const rest = form.picks.filter((pick) => pickKey(pick) !== key);
    onChange(on ? [...rest, { project, kind, name, access: "read" }] : rest);
  };
  const setAccess = (pick: Pick, access: "read" | "write") =>
    onChange(form.picks.map((one) => (pickKey(one) === pickKey(pick) ? { ...one, access } : one)));
  const row = (project: string, kind: PickKind, name: string | undefined, label: string, disabledReason?: string, readOnly = false) => {
    const pick = picked(form, project, kind, name);
    // A ticked row stays enabled so an edit can drop an item the ceiling no longer allows.
    const reason = pick ? undefined : disabledReason;
    return (
      <li key={`${kind}/${name ?? ""}`} className="flex flex-wrap items-center gap-2">
        <Checkbox
          label={label}
          checked={Boolean(pick)}
          disabled={Boolean(reason)}
          disabledReason={reason}
          onChange={(event) => toggle(project, kind, name, event.target.checked)}
        />
        {pick && !readOnly ? (
          <Select
            aria-label={t("aiConnectors.accessOf", { what: label })}
            value={pick.access}
            onChange={(event) => setAccess(pick, event.target.value as "read" | "write")}
            className="w-auto"
          >
            <option value="read">{t("aiConnectors.access.read")}</option>
            <option value="write" disabled={!options.ceiling.writes}>
              {options.ceiling.writes ? t("aiConnectors.access.write") : t("aiConnectors.access.writeNeedsApproval")}
            </option>
          </Select>
        ) : null}
      </li>
    );
  };
  return (
    <>
      {searchable ? (
        <Field id="connector-find" label={t("aiConnectors.find")} help={t("aiConnectors.findHint")}>
          <Input id="connector-find" type="search" value={find} onChange={(event) => setFind(event.target.value)} />
        </Field>
      ) : null}
      <ul className="flex flex-col gap-3" aria-label={t("aiConnectors.field.scope")}>
        {options.projects.map((project) => {
          const whole = Boolean(picked(form, project.project, "project"));
          const inWhole = whole ? t("aiConnectors.inWholeProject") : undefined;
          const onlyEndpoints = project.allowedEndpoints ?? undefined;
          // Organization data (ADR-N-067 §2.4): Endpoints shared with the person, read only, never the project.
          const shared = project.organizationData;
          // A hint only: Create checks every item against the ceiling again (T-6307).
          const refused = !project.allowed
            ? t("aiConnectors.projectAboveCeiling")
            : shared
              ? t("aiConnectors.organizationDataHint")
              : onlyEndpoints
                ? t("aiConnectors.endpointsOnly")
                : undefined;
          const open = (name: string) => project.allowed && (onlyEndpoints ?? (shared ? [name] : [])).includes(name);
          const reason = (kind: PickKind, name: string) => (kind === "endpoint" && open(name) ? inWhole : (refused ?? inWhole));
          const groups: [PickKind, string[]][] = [
            ["space", project.spaces],
            ["endpoint", project.endpoints],
            ["pipeline", project.pipelines],
          ];
          const found = groups.map(([kind, names]): [PickKind, string[]] => [
            kind,
            names.filter((name) => !word || project.project.includes(word) || name.toLowerCase().includes(word)),
          ]);
          const shown = found.reduce((sum, [, names]) => sum + names.length, 0);
          if (word && shown === 0) return null;
          const items = found.map(([kind, names]) =>
            names.length > 0 ? (
              <fieldset key={kind} className="ml-6 mt-2">
                <legend className={clsx("text-caption font-semibold", !project.allowed && "text-fg-muted")}>{t(`aiConnectors.kinds.${kind}`)}</legend>
                <ul className="flex flex-col gap-1">{names.map((name) => row(project.project, kind, name, name, reason(kind, name), shared))}</ul>
              </fieldset>
            ) : null,
          );
          const ticked = form.picks.some((pick) => pick.project === project.project && pick.kind !== "project");
          return (
            <li key={project.project} className="rounded-md border border-border p-2">
              {shared ? (
                <p className="text-body font-semibold">{t("aiConnectors.organizationData", { project: project.project })}</p>
              ) : (
                <ul className="flex flex-col gap-1">
                  {row(project.project, "project", undefined, t("aiConnectors.wholeProject", { project: project.project }), refused)}
                </ul>
              )}
              {refused ? (
                <p className={clsx("text-caption", shared ? "text-fg-muted" : "ml-6 text-fg")} aria-hidden={!shared}>
                  {refused}
                </p>
              ) : null}
              {sizeOf(project) > FOLD_OVER && !word ? (
                <details open={ticked} className="ml-6 mt-2">
                  <summary className="focus-ring w-fit cursor-pointer rounded-sm text-caption font-semibold">
                    {t("aiConnectors.narrow", { count: sizeOf(project) })}
                  </summary>
                  {items}
                </details>
              ) : (
                items
              )}
            </li>
          );
        })}
      </ul>
    </>
  );
}

/** A new connector, or an edit of `stored`: one form, checked by the server again on submit. */
/**
 * The verbs per kind under the ticked presets (T-6302): read comes with the preset, create, edit,
 * delete and (on Pipeline) run are ticked one by one, nothing by default; a verb the ceiling
 * withholds is greyed with the reason.
 */
/** A refusal shown beside its section reads without the field path the server leads with (`kinds[2].verbs: `). */
function withoutField(message: string): string {
  return message.replace(/^[A-Za-z]+(\[\d+\])?(\.[A-Za-z]+)?: /, "");
}

/** The kind a refusal of `kinds[i]...` names, read from the request the form sent. */
function refusedKind(field: string | undefined, form: AiConnectorForm, options: ConnectorOptions): string | undefined {
  const at = /^kinds\[(\d+)\]/.exec(field ?? "")?.[1];
  return at === undefined ? undefined : toRequest(form, options).kinds?.[Number(at)]?.kind;
}

/** `Endpoint: create`, one per kind with a verb beyond read; none for a connector without a list. */
function changeLines(connector: AiConnector, word: (verb: string) => string): string[] {
  return (connector.kinds ?? [])
    .map((one) => ({ kind: one.kind, verbs: one.verbs.filter((verb) => verb !== "read") }))
    .filter((one) => one.verbs.length > 0)
    .map((one) => `${one.kind}: ${one.verbs.map(word).join(", ")}`);
}

export function KindVerbsPicker({
  options,
  form,
  onChange,
  refusal,
}: {
  options: ConnectorOptions;
  form: AiConnectorForm;
  onChange: (verbs: Record<string, ChangeVerb[]>) => void;
  /** A server refusal of the list: beside its kind's row, or under the rows when it names none. */
  refusal?: { kind?: string; message: string };
}): JSX.Element {
  const { t } = useTranslation();
  const rows = kindRows(options, form.segments);
  if (rows.length === 0) return <p className="text-caption text-fg-muted">{t("aiConnectors.verbsNone")}</p>;
  const toggle = (kind: string, verb: ChangeVerb, on: boolean) => {
    const held = form.verbs[kind] ?? [];
    onChange({ ...form.verbs, [kind]: on ? [...held, verb] : held.filter((one) => one !== verb) });
  };
  const greyed = rows.some((row) => row.kinds.some((kind) => verbsOf(kind).some((verb) => !ceilingAllows(options, kind, verb))));
  const label = (kind: string) => {
    const word = t(`kinds.${kind}.other`, { defaultValue: kind });
    return word.charAt(0).toUpperCase() + word.slice(1);
  };
  return (
    <div className="flex flex-col gap-2">
      {rows.map((row) => (
        <div key={row.segment} className="flex flex-col">
          <p className="text-body font-semibold">{t(`aiConnectors.segment.${row.segment}`)}</p>
          {row.kinds.map((kind) => (
            <Fragment key={kind}>
              <fieldset className="flex flex-wrap items-center gap-x-3 border-t border-border py-0.5">
                <legend className="sr-only">{t("aiConnectors.verbsOf", { kind: label(kind) })}</legend>
                <span aria-hidden className="w-full text-body font-medium sm:w-36">
                  {label(kind)}
                </span>
                <Checkbox label={t("aiConnectors.verb.read")} checked readOnly disabled disabledReason={t("aiConnectors.readComes")} />
                {verbsOf(kind).map((verb) => {
                  const allowed = ceilingAllows(options, kind, verb);
                  return (
                    <Checkbox
                      key={verb}
                      label={t(`aiConnectors.verb.${verb}`)}
                      checked={allowed && (form.verbs[kind]?.includes(verb) ?? false)}
                      disabled={!allowed}
                      disabledReason={allowed ? undefined : t("aiConnectors.aboveCeiling")}
                      onChange={(event) => toggle(kind, verb, event.target.checked)}
                    />
                  );
                })}
              </fieldset>
              {refusal?.kind === kind ? <Alert tone="danger">{refusal.message}</Alert> : null}
            </Fragment>
          ))}
        </div>
      ))}
      {refusal && !rows.some((row) => refusal.kind !== undefined && row.kinds.includes(refusal.kind)) ? <Alert tone="danger">{refusal.message}</Alert> : null}
      {greyed ? <p className="text-caption text-fg">{t("aiConnectors.verbsGreyed")}</p> : null}
    </div>
  );
}

/** The presets: every one with its hint, or for a member the ones they hold and one line for the rest. */
export function PresetsField({
  options,
  form,
  readsOnly,
  errors,
  onChange,
}: {
  options: ConnectorOptions;
  form: AiConnectorForm;
  /** A member over shared datasets alone, who can do nothing about a greyed preset here. */
  readsOnly: boolean;
  errors?: string[];
  onChange: (segments: string[]) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const { ceiling } = options;
  const offered = options.segments.filter((segment) => !readsOnly || ceiling.segments.includes(segment) || form.segments.includes(segment));
  const withheld = options.segments.length - offered.length;
  const hintOf = (segment: string) => {
    if (!ceiling.segments.includes(segment)) return t("aiConnectors.needsApproval");
    return t(readsOnly && segment === "data-read" ? "aiConnectors.segmentHintShared" : `aiConnectors.segmentHint.${segment}`);
  };
  return (
    <fieldset className="flex flex-col gap-1" aria-describedby="ai-connector-segments-help">
      <legend className="text-body font-semibold">{t("aiConnectors.field.segments")}</legend>
      <p id="ai-connector-segments-help" className="text-caption text-fg-muted">
        {t(readsOnly ? "aiConnectors.help.segmentsShared" : "aiConnectors.help.segments")}
      </p>
      {offered.map((segment) => {
        const allowed = ceiling.segments.includes(segment);
        return (
          <Checkbox
            key={segment}
            label={t(`aiConnectors.segment.${segment}`)}
            className="flex-wrap gap-y-0 max-sm:[&>span+span]:basis-full"
            hint={hintOf(segment)}
            checked={form.segments.includes(segment)}
            disabled={!allowed}
            disabledReason={allowed ? undefined : t("aiConnectors.aboveCeiling")}
            onChange={(event) => onChange(event.target.checked ? [...form.segments, segment] : form.segments.filter((one) => one !== segment))}
          />
        );
      })}
      {withheld > 0 ? <p className="text-caption text-fg">{t("aiConnectors.otherPresets", { count: withheld })}</p> : null}
      {errors ? <Alert tone="danger">{errors[0]}</Alert> : null}
    </fieldset>
  );
}

/**
 * The clients a connector answers. A refusal sits under the client it names, which is marked, and
 * leaves when that client is unticked; a hint every row would repeat goes in the line above them.
 */
export function ClientsField({
  options,
  form,
  refused,
  onChange,
}: {
  options: ConnectorOptions;
  form: AiConnectorForm;
  /** The server's refusal of the list. */
  refused?: string;
  onChange: (clients: string[]) => void;
}): JSX.Element {
  const { t } = useTranslation();
  // The refusal's place is below the fold: bring it into view once.
  const shown = useCallback((alert: HTMLDivElement | null) => alert?.scrollIntoView?.({ block: "center" }), []);
  const said = refused ? clientRefusal(refused, options.clients, t) : undefined;
  const refusal = said?.id && !form.clients.includes(said.id) ? undefined : said;
  const alert = refusal ? (
    <div ref={shown}>
      <Alert tone="danger">{refusal.text}</Alert>
    </div>
  ) : null;
  const listed = options.clients.some((one) => one.refused);
  const sameHint = options.clients.every((one) => !one.refused && !one.isBoundary);
  const marked = (one: ConnectorOptions["clients"][number]) => one.id === refusal?.id || (Boolean(one.refused) && form.clients.includes(one.id));
  const hintOf = (one: ConnectorOptions["clients"][number]) => {
    if (marked(one)) return <Badge tone="danger">{t("aiConnectors.clientNotAllowed")}</Badge>;
    // The line above says why a client is greyed; a hint every row would repeat stays there.
    if (one.refused || sameHint) return undefined;
    return one.isBoundary ? t("aiConnectors.binds") : t("aiConnectors.intendedFor");
  };
  return (
    <fieldset className="flex flex-col gap-1" aria-describedby="ai-connector-clients-help">
      <legend className="text-body font-semibold">{t("aiConnectors.field.clients")}</legend>
      <p id="ai-connector-clients-help" className="text-caption text-fg-muted">
        {t(listed ? "aiConnectors.help.clientsListed" : sameHint ? "aiConnectors.help.clientsSayWhom" : "aiConnectors.help.clients")}
      </p>
      {options.clients.map((one) => {
        const ticked = form.clients.includes(one.id);
        return (
          <Fragment key={one.id}>
            <Checkbox
              label={clientLabel(one.id, t)}
              hint={hintOf(one)}
              // The badge stays beside its client on a phone; a hint takes the line under it.
              className={clsx("flex-wrap gap-y-0", !marked(one) && "max-sm:[&>span+span]:basis-full")}
              disabled={Boolean(one.refused) && !ticked}
              disabledReason={one.refused ? t("aiConnectors.clientRefusedWhy") : undefined}
              aria-invalid={marked(one) || undefined}
              checked={ticked}
              onChange={(event) => onChange(event.target.checked ? [...form.clients, one.id] : form.clients.filter((id) => id !== one.id))}
            />
            {one.id === refusal?.id ? alert : null}
          </Fragment>
        );
      })}
      {refusal && !refusal.id ? alert : null}
    </fieldset>
  );
}

/** Nobody can tick anything, so no form: what to do next, and Create off with its reason. */
export function NothingToReach({ onClose }: { onClose: () => void }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Dialog
      open
      onOpenChange={(open) => (open ? undefined : onClose())}
      size="lg"
      title={t("aiConnectors.newTitle")}
      closeLabel={t("app.close")}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("form.cancel")}
          </Button>
          <Button disabled disabledReason={t("aiConnectors.nothingToReach")}>
            {t("aiConnectors.create")}
          </Button>
        </>
      }
    >
      <Alert tone="info">{t("aiConnectors.noProjects")}</Alert>
    </Dialog>
  );
}

export function CreateDialog({
  stored,
  onClose,
  onCreated,
}: {
  stored?: AiConnector;
  onClose: () => void;
  onCreated: (id: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const options = useOptions();
  const [now] = useState(() => new Date());
  const [form, setForm] = useState<AiConnectorForm | null>(null);
  const [tried, setTried] = useState(false);
  const [refused, setRefused] = useState<{ section?: string; field?: string; message: string } | null>(null);
  const current = form ?? (stored ? toForm(stored, options.data) : emptyForm(options.data, now));
  const set = (patch: Partial<AiConnectorForm>) => setForm({ ...current, ...patch });
  const problems = options.data ? formProblems(current, options.data, now) : {};
  const create = useMutation({
    mutationFn: async () => {
      const request = toRequest(current, options.data);
      const { segments, scope, kinds, writes, autonomous, expiresAt, allowedClients } = request;
      return stored
        ? unwrap(
            await api.PATCH("/api/v1/connectors/{id}", {
              params: { path: { id: stored.id } },
              body: { segments, scope, kinds, writes, autonomous, expiresAt, allowedClients },
            }),
          )
        : unwrap(await api.POST("/api/v1/connectors", { body: request }));
    },
    onSuccess: (connector) => {
      void client.invalidateQueries({ queryKey: KEY });
      // A create without `?dryRun` answers the connector; the dry run's `{valid}` is never asked here.
      if ("id" in connector) onCreated(connector.id);
    },
    onError: (error: Error) => {
      const problem = error instanceof ApiError ? error.problem : undefined;
      const message = problem?.detail ?? error.message;
      setRefused({ section: refusedSection(problem?.errors ?? undefined, message), field: problem?.errors?.[0] ?? /^\S+(?=: )/.exec(message)?.[0], message });
    },
  });
  const errorsOf = (section: string) =>
    refused?.section === section ? [withoutField(refused.message)] : tried && problems[section] ? [t(problems[section])] : undefined;
  const submit = () => {
    setTried(true);
    setRefused(null);
    if (Object.keys(problems).length === 0) create.mutate();
  };
  const ceiling = options.data?.ceiling;
  // A member reaches only datasets shared with them, read only: no change verbs to choose (T-6413).
  const readsOnly = Boolean(options.data?.projects.every((project) => project.organizationData));
  const nothing = !stored && options.data?.projects.length === 0;
  const clientRefused = refused?.section === "allowedClients" ? refused.message : undefined;
  if (nothing) return <NothingToReach onClose={onClose} />;
  const stop = options.data ? refusalHolding(clientRefused, current, options.data.clients, t) : t("aiConnectors.loadingOptions");
  return (
    <Dialog
      open
      onOpenChange={(open) => (open ? undefined : onClose())}
      size="lg"
      title={stored ? t("aiConnectors.editTitle", { name: stored.name }) : t("aiConnectors.newTitle")}
      description={t("aiConnectors.formLead")}
      closeLabel={t("app.close")}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("form.cancel")}
          </Button>
          <Button loading={create.isPending} disabled={Boolean(stop)} disabledReason={stop} onClick={submit}>
            {stored ? t("aiConnectors.save") : t("aiConnectors.create")}
          </Button>
        </>
      }
    >
      {options.isPending ? <PageLoading label={t("aiConnectors.loadingOptions")} /> : null}
      {options.isError ? <PageFailed error={options.error} onRetry={() => void options.refetch()} /> : null}
      {options.data && ceiling ? (
        <div className="flex flex-col gap-4">
          <Field
            id="ai-connector-name"
            label={t("aiConnectors.field.name")}
            required
            help={stored ? t("aiConnectors.help.nameStays") : t("aiConnectors.help.name")}
            errors={errorsOf("name")}
          >
            <Input
              id="ai-connector-name"
              value={current.name}
              readOnly={Boolean(stored)}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => set({ name: event.target.value.trim() })}
            />
          </Field>
          <PresetsField options={options.data} form={current} readsOnly={readsOnly} errors={errorsOf("segments")} onChange={(segments) => set({ segments })} />
          {readsOnly ? null : (
            <fieldset className="flex flex-col gap-1" aria-describedby="ai-connector-kinds-help">
              <legend className="text-body font-semibold">{t("aiConnectors.field.kinds")}</legend>
              <KindVerbsPicker
                options={options.data}
                form={current}
                onChange={(verbs) => set({ verbs })}
                refusal={refused?.section === "kinds" ? { kind: refusedKind(refused.field, current, options.data), message: withoutField(refused.message) } : undefined}
              />
              <p id="ai-connector-kinds-help" className="text-caption text-fg-muted">
                {t("aiConnectors.help.kinds")}
              </p>
            </fieldset>
          )}
          <fieldset className="flex flex-col gap-1" aria-describedby="ai-connector-scope-help">
            <legend className="text-body font-semibold">{t("aiConnectors.field.scope")}</legend>
            <p id="ai-connector-scope-help" className="text-caption text-fg-muted">
              {t(readsOnly ? "aiConnectors.help.scopeShared" : "aiConnectors.help.scope")}
            </p>
            <ScopeTree options={options.data} form={current} onChange={(picks) => set({ picks })} />
            {errorsOf("scope") ? <Alert tone="danger">{errorsOf("scope")?.[0]}</Alert> : null}
          </fieldset>
          {readsOnly ? null : (
            <fieldset className="flex flex-col gap-1">
              <legend className="text-body font-semibold">{t("aiConnectors.field.autonomous")}</legend>
              <Checkbox
                label={t("aiConnectors.autonomous.label")}
                hint={t("aiConnectors.autonomous.hint")}
                checked={current.autonomous && current.picks.some((pick) => pick.access === "write")}
                disabled={!current.picks.some((pick) => pick.access === "write")}
                disabledReason={t("aiConnectors.autonomous.needsWrite")}
                onChange={(event) => set({ autonomous: event.target.checked })}
              />
              {errorsOf("autonomous") ? <Alert tone="danger">{errorsOf("autonomous")?.[0]}</Alert> : null}
            </fieldset>
          )}
          <Field
            id="ai-connector-expires"
            label={t("aiConnectors.field.expires")}
            required
            help={t("aiConnectors.help.expires", { days: ceiling.maxDays })}
            errors={errorsOf("expiresAt")}
          >
            <Input
              id="ai-connector-expires"
              type="date"
              value={current.expires}
              min={now.toISOString().slice(0, 10)}
              max={lastDay(ceiling.maxDays, now)}
              onChange={(event) => set({ expires: event.target.value })}
            />
          </Field>
          <ClientsField
            options={options.data}
            form={current}
            refused={clientRefused}
            onChange={(clients) => set({ clients })}
          />
          {refused && !refused.section ? (
            <Alert tone="danger" role="alert">
              {refused.message}
            </Alert>
          ) : null}
        </div>
      ) : null}
    </Dialog>
  );
}
