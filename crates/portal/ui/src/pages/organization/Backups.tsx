import { useState } from "react";
import type { FormEvent, JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { ApiError, reasonOf } from "../../api/client";
import { door, doorAction } from "../../api/odata";
import { useAdministers } from "../../api/permissions";
import { AdminGrid } from "../../components/grid/AdminGrid";
import { useIdentity } from "../../auth/AuthProvider";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Button,
  Card,
  CardHeader,
  ConfirmDialog,
  Dialog,
  EmptyState,
  Field,
  Input,
  Select,
  Switch,
  Textarea,
} from "../../components/ui";

type Overview = components["schemas"]["BackupOverview"];
type Target = components["schemas"]["Target"];
type VersionList = components["schemas"]["VersionList"];
type RestorePlan = components["schemas"]["RestorePlan"];
type DataRestoreAnswer = components["schemas"]["DataRestoreAnswer"];
type RestoreRequest = components["schemas"]["DataRestoreRequest"];
type DataPlan = components["schemas"]["DataPlan"];
type ArchivedProjects = components["schemas"]["ArchivedProjects"];
type CopyPlan = components["schemas"]["CopyPlan"];

/** The configuration backup (manifests, T-7154) and the data backup (database rows, encrypted, T-7168). */
type Kind = "configuration" | "data";
const overviewKey = (kind: Kind) => ["organizationBackup", kind] as const;
const versionsKey = (kind: Kind) => ["organizationBackup", kind, "versions"] as const;
const RESTORES = ["organizationBackup", "data", "restores"] as const;
/** The restore plan group of the bundle's people (T-7164): listed, never created. */
const PEOPLE = "organization/people";

// Each backup's doors (API/13 §2b): the Portal answers both through OData.
async function readOverview(kind: Kind): Promise<Overview> {
  return (await door(kind === "data" ? "portal_backup_data_get" : "portal_backup_configuration_get", {})) as Overview;
}

async function runNow(kind: Kind): Promise<unknown> {
  return doorAction(kind === "data" ? "portal_backup_data_run" : "portal_backup_configuration_run", {});
}

async function readVersions(kind: Kind): Promise<VersionList> {
  return (await door(kind === "data" ? "portal_backup_data_versions" : "portal_backup_configuration_versions", {})) as VersionList;
}

async function softDelete(kind: Kind): Promise<unknown> {
  return doorAction(kind === "data" ? "portal_backup_data_delete" : "portal_backup_configuration_delete", {});
}

/** One version's bytes: the door answers the file as its base64 (OData answers JSON only). */
async function downloadVersion(kind: Kind, version: string, unreadable: string): Promise<Blob> {
  const base64 = await door(kind === "data" ? "portal_backup_data_download" : "portal_backup_configuration_download", { version });
  if (typeof base64 !== "string" || base64 === "") throw new ApiError(502, unreadable);
  const type = kind === "data" ? "application/octet-stream" : "application/zip";
  return new Blob([Uint8Array.from(atob(base64), (c) => c.charCodeAt(0))], { type });
}

/** The form's fields, in the server's names: a 422 names one of them and the page marks it. */
const FIELDS = [
  "endpoint",
  "region",
  "bucket",
  "prefix",
  "accessKeyId",
  "secretAccessKey",
  "everyHours",
  "keepDays",
  "workingRecipient",
  "recoveryRecipient",
] as const;
type FieldName = (typeof FIELDS)[number];

interface Form {
  endpoint: string;
  region: string;
  bucket: string;
  prefix: string;
  accessKeyId: string;
  secretAccessKey: string;
  enabled: boolean;
  everyHours: string;
  keepDays: string;
  workingRecipient: string;
  recoveryRecipient: string;
}

function formOf(target: Target | null | undefined): Form {
  return {
    endpoint: target?.endpoint ?? "",
    region: target?.region ?? "",
    bucket: target?.bucket ?? "",
    prefix: target?.prefix ?? "",
    accessKeyId: target?.accessKeyId ?? "",
    secretAccessKey: "",
    enabled: target?.enabled ?? false,
    everyHours: String(target?.everyHours ?? 24),
    keepDays: String(target?.keepDays ?? 90),
    workingRecipient: target?.recipients[0] ?? "",
    recoveryRecipient: target?.recipients[1] ?? "",
  };
}

/** The field a refusal names (`bucket: 3 to 63 …`) and its reason without the name. */
function fieldError(error: unknown): { field: FieldName; reason: string } | null {
  if (!(error instanceof ApiError) || error.status !== 422) return null;
  const line = error.problem?.errors?.[0] ?? error.problem?.detail ?? "";
  const field = FIELDS.find((name) => line.startsWith(`${name}:`));
  return field ? { field, reason: line.slice(field.length + 1).trim() } : null;
}

function bytes(size: number, locale: string): string {
  if (size < 1024) return `${size} B`;
  const units = ["KiB", "MiB", "GiB"];
  let value = size / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toLocaleString(locale, { maximumFractionDigits: 1 })} ${units[unit]}`;
}

function when(at: string | null | undefined, locale: string): string {
  if (!at) return "";
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString(locale, { dateStyle: "medium", timeStyle: "short" });
}

/**
 * Administration → Backups (T-7154): the Organization's configuration backup to its own S3
 * bucket. Where it goes and when, the bucket's versioning, the last runs, and every version with
 * download, soft delete and restore. Only the Organization's administrators open it; the server
 * refuses anyone else, and the secret key it is given never comes back.
 */
export function Backups(): JSX.Element {
  const { t } = useTranslation();
  const { known, administers, own } = useAdministers();
  const allowed = administers || own !== undefined;
  if (!known) return <p className="text-body text-fg-muted">{t("backups.loading")}</p>;
  if (!allowed) return <Alert tone="info">{t("organization.adminOnly")}</Alert>;
  return (
    <div className="flex flex-col gap-10" data-testid="organization-backups">
      <nav aria-label={t("backups.section.jump")} className="flex flex-wrap gap-x-4 gap-y-1 text-body">
        <a className="focus-ring text-primary-soft-fg underline hover:no-underline" href="#backup-configuration-title">
          {t("backups.section.configuration")}
        </a>
        <a className="focus-ring text-primary-soft-fg underline hover:no-underline" href="#backup-data-title">
          {t("backups.section.data")}
        </a>
      </nav>
      <Section kind="configuration" />
      <Section kind="data" />
    </div>
  );
}

/** One backup: its runs, its versions and its bucket form. */
function Section({ kind }: { kind: Kind }): JSX.Element {
  const { t } = useTranslation();
  const overview = useQuery({ queryKey: overviewKey(kind), queryFn: () => readOverview(kind) });
  const target = overview.data?.target ?? null;
  return (
    <section className="flex flex-col gap-6" aria-labelledby={`backup-${kind}-title`} data-testid={`backup-${kind}`}>
      <div className="flex flex-col gap-1">
        <h2 id={`backup-${kind}-title`} className="text-title font-semibold">
          {t(`backups.section.${kind}`)}
        </h2>
        <p className="text-body text-fg-muted">{kind === "data" ? t("backups.data.lead") : t("backups.lead")}</p>
      </div>
      {overview.isLoading ? <p className="text-body text-fg-muted">{t("backups.loading")}</p> : null}
      {overview.isError ? <Alert tone="danger">{reasonOf(overview.error, t("backups.unreadable"))}</Alert> : null}
      {target ? <Runs kind={kind} target={target} overview={overview.data} /> : null}
      {target ? <Versions kind={kind} /> : null}
      {target && kind === "data" ? <RestoreRequests /> : null}
      {overview.data ? (
        <TargetForm key={target?.updatedAt ?? "new"} kind={kind} target={target} fingerprints={overview.data.fingerprints} />
      ) : null}
    </section>
  );
}

function Runs({ kind, target, overview }: { kind: Kind; target: Target; overview: Overview | undefined }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const client = useQueryClient();
  const run = useMutation({
    mutationFn: () => runNow(kind),
    onSettled: async () => client.invalidateQueries({ queryKey: overviewKey(kind) }),
  });
  const log = overview?.log ?? [];
  const lastRun = log.find((entry) => entry.action === "run");
  return (
    <Card>
      <CardHeader
        title={t("backups.runs.title")}
        description={
          target.enabled
            ? t("backups.runs.scheduled", { hours: target.everyHours })
            : t("backups.runs.off")
        }
        actions={
          <Button variant="primary" onClick={() => run.mutate()} disabled={run.isPending} disabledReason={t("app.busyReason")}>
            {run.isPending ? t("backups.runs.running") : t("backups.runs.now")}
          </Button>
        }
      />
      {run.isError ? <Alert tone="danger">{reasonOf(run.error, t("backups.runs.failed"))}</Alert> : null}
      {run.isSuccess && log[0]?.action === "run" ? <Alert tone="success">{t("backups.runs.done")}</Alert> : null}
      {lastRun ? (
        <p className="text-body" data-testid="backup-last-run">
          {lastRun.ok
            ? t("backups.runs.lastOk", { at: when(lastRun.at, locale), size: bytes(lastRun.bytes, locale), by: lastRun.actor })
            : t("backups.runs.lastFailed", { at: when(lastRun.at, locale), reason: lastRun.message, by: lastRun.actor })}
        </p>
      ) : (
        <p className="text-body text-fg-muted">{t("backups.runs.never")}</p>
      )}
      {log.length > 0 ? (
        <details className="mt-3">
          <summary className="cursor-pointer text-body text-fg-muted">{t("backups.log.title", { count: log.length })}</summary>
          <ul className="mt-2 flex flex-col gap-1 text-caption">
            {log.map((entry) => (
              <li key={entry.id} className="flex flex-wrap gap-x-2">
                <span className="text-fg-muted">{when(entry.at, locale)}</span>
                <span>{t(`backups.log.action.${entry.action === "restore-plan" ? "plan" : entry.action}`)}</span>
                <span className="text-fg-muted">{entry.actor}</span>
                <Badge tone={entry.ok ? "success" : "danger"}>{entry.ok ? t("backups.log.ok") : t("backups.log.failed")}</Badge>
                {entry.ok ? null : <span className="text-fg-muted">{entry.message}</span>}
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </Card>
  );
}

function Versions({ kind }: { kind: Kind }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const client = useQueryClient();
  const [deleting, setDeleting] = useState(false);
  const [restoring, setRestoring] = useState<string | null>(null);
  const [copying, setCopying] = useState<string | null>(null);
  const versions = useQuery({ queryKey: versionsKey(kind), queryFn: () => readVersions(kind) });
  const remove = useMutation({
    mutationFn: () => softDelete(kind),
    onSettled: async () => {
      setDeleting(false);
      await client.invalidateQueries({ queryKey: overviewKey(kind) });
      await client.invalidateQueries({ queryKey: versionsKey(kind) });
    },
  });
  const save = async (version: string) => {
    const blob = await downloadVersion(kind, version, t("backups.unreadable"));
    const href = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = href;
    link.download = kind === "data" ? `data-${version}.zip.age` : `configuration-${version}.zip`;
    link.click();
    URL.revokeObjectURL(href);
  };
  const list = versions.data?.versions ?? [];
  const current = list.find((version) => version.isLatest);
  return (
    <Card>
      <CardHeader
        title={t("backups.versions.title")}
        description={kind === "data" ? t("backups.data.versionsLead") : t("backups.versions.lead")}
        actions={
          current && !current.deleteMarker ? (
            <Button variant="danger" onClick={() => setDeleting(true)}>
              {t("backups.versions.delete")}
            </Button>
          ) : undefined
        }
      />
      {versions.isLoading ? <p className="text-body text-fg-muted">{t("backups.loading")}</p> : null}
      {versions.isError ? <Alert tone="danger">{reasonOf(versions.error, t("backups.unreadable"))}</Alert> : null}
      {versions.data ? (
        <div className="mb-3 flex flex-wrap gap-2">
          <Badge tone={versions.data.versioning === "Enabled" ? "success" : "danger"}>
            {versions.data.versioning === "Enabled" ? t("backups.versions.versioningOn") : t("backups.versions.versioningOff")}
          </Badge>
          <Badge tone={versions.data.objectLock ? "success" : "neutral"}>
            {versions.data.objectLock ? t("backups.versions.lockOn") : t("backups.versions.lockOff")}
          </Badge>
        </div>
      ) : null}
      {current?.deleteMarker ? <Alert tone="warning">{t("backups.versions.deletedHint")}</Alert> : null}
      {versions.data && list.length === 0 ? (
        <EmptyState bare title={t("backups.versions.empty")} description={t("backups.versions.emptyHint")} />
      ) : null}
      {list.length > 0 ? (
        <ul className="flex flex-col divide-y divide-border" aria-label={t("backups.versions.title")}>
          {list.map((version) => (
            <li key={version.versionId} className="flex flex-wrap items-center justify-between gap-2 py-2">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-body">{when(version.lastModified, locale)}</span>
                {version.deleteMarker ? (
                  <Badge tone="warning">{t("backups.versions.deleted")}</Badge>
                ) : (
                  <span className="text-caption text-fg-muted">{bytes(version.size, locale)}</span>
                )}
                {version.isLatest && !version.deleteMarker ? <Badge tone="primary">{t("backups.versions.current")}</Badge> : null}
              </div>
              {version.deleteMarker ? null : (
                <div className="flex flex-wrap gap-2">
                  <Button size="sm" variant="secondary" onClick={() => void save(version.versionId)}>
                    {t("backups.versions.download")}
                  </Button>
                  <Button size="sm" variant="secondary" onClick={() => setRestoring(version.versionId)}>
                    {t("backups.versions.restore")}
                  </Button>
                  {kind === "data" ? (
                    <Button size="sm" variant="secondary" onClick={() => setCopying(version.versionId)}>
                      {t("backups.copy.action")}
                    </Button>
                  ) : null}
                </div>
              )}
            </li>
          ))}
        </ul>
      ) : null}
      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={t("backups.versions.deleteTitle")}
        description={t("backups.versions.deleteHint")}
        confirmLabel={t("backups.versions.delete")}
        pending={remove.isPending}
        onConfirm={() => remove.mutate()}
      />
      {restoring && kind === "configuration" ? <Restore version={restoring} onClose={() => setRestoring(null)} /> : null}
      {restoring && kind === "data" ? <DataRestore version={restoring} onClose={() => setRestoring(null)} /> : null}
      {copying && kind === "data" ? <ProjectCopy version={copying} onClose={() => setCopying(null)} /> : null}
    </Card>
  );
}

function Restore({ version, onClose }: { version: string; onClose: () => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const client = useQueryClient();
  const call = async (apply: boolean): Promise<RestorePlan> =>
    (await doorAction("portal_backup_configuration_restore", { version, body: { apply } })) as RestorePlan;
  const plan = useQuery({ queryKey: ["organizationBackup", "plan", version], queryFn: () => call(false) });
  const apply = useMutation({
    mutationFn: () => call(true),
    onSettled: async () => client.invalidateQueries({ queryKey: overviewKey("configuration") }),
  });
  const shown = apply.data ?? plan.data;
  const projectChanges = (shown?.groups ?? []).filter(
    (group) => group.name !== "organization" && group.name !== PEOPLE && group.created.length + group.changed.length > 0,
  );
  const toInvite = shown?.groups.find((group) => group.name === PEOPLE)?.created.length ?? 0;
  return (
    <Dialog
      open
      onOpenChange={(open) => (open ? undefined : onClose())}
      title={t("backups.restore.title")}
      closeLabel={t("backups.restore.close")}
      description={shown ? t("backups.restore.taken", { at: when(shown.takenAt, locale) }) : undefined}
    >
      <div className="flex flex-col gap-3" data-testid="backup-restore-plan">
        {plan.isLoading ? <p className="text-body text-fg-muted">{t("backups.restore.planning")}</p> : null}
        {plan.isError ? <Alert tone="danger">{reasonOf(plan.error, t("backups.unreadable"))}</Alert> : null}
        {apply.isError ? <Alert tone="danger">{reasonOf(apply.error, t("backups.unreadable"))}</Alert> : null}
        {shown && shown.changes === 0 ? (
          <Alert tone={toInvite > 0 ? "info" : "success"}>
            {t(toInvite > 0 ? "backups.restore.sameButPeople" : "backups.restore.same")}
          </Alert>
        ) : null}
        {shown?.groups.map((group) => (
          <section key={group.name} className="flex flex-col gap-1">
            <h3 className="text-body font-semibold">
              {group.name === "organization"
                ? t("backups.restore.organization")
                : group.name === PEOPLE
                  ? t("backups.restore.people")
                  : t("backups.restore.project", { project: group.name })}
            </h3>
            <p className="text-body text-fg-muted">
              {group.name === PEOPLE
                ? t("backups.restore.peopleCounts", {
                    missing: group.created.length,
                    present: group.unchanged,
                    total: group.created.length + group.unchanged,
                  })
                : t("backups.restore.counts", {
                    created: group.created.length,
                    changed: group.changed.length,
                    unchanged: group.unchanged,
                  })}
            </p>
            {[...group.created, ...group.changed].length > 0 ? (
              <ul className="list-disc pl-5 text-body">
                {[...group.created, ...group.changed].map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            ) : null}
            {group.note && group.name === PEOPLE ? (
              <p className="text-body">
                {t("backups.restore.peopleNote")}{" "}
                <Link to="/organization/$tab" params={{ tab: "people" }} className="text-body font-semibold underline hover:no-underline">
                  {t("backups.restore.invite")}
                </Link>
              </p>
            ) : null}
            {group.note && group.name !== PEOPLE ? (
              <p className="text-caption text-fg-muted">{t("backups.restore.organizationNote")}</p>
            ) : null}
            {group.change ? <Alert tone="success">{t("backups.restore.proposed")}</Alert> : null}
          </section>
        ))}
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            {t("backups.restore.close")}
          </Button>
          {projectChanges.length > 0 && !apply.data ? (
            <Button variant="primary" onClick={() => apply.mutate()} disabled={apply.isPending} disabledReason={t("app.busyReason")}>
              {t("backups.restore.propose")}
            </Button>
          ) : null}
        </div>
      </div>
    </Dialog>
  );
}

/** The identity field of a data restore: a password field, never kept past the request. */
function IdentityField({ id, value, onChange, error }: { id: string; value: string; onChange: (v: string) => void; error?: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Field id={id} label={t("backups.data.identity")} help={t("backups.data.identityHint")} errors={error ? [error] : undefined}>
      <Input id={id} type="password" autoComplete="off" spellCheck={false} value={value} onChange={(e) => onChange(e.target.value)} />
    </Field>
  );
}

/** The reason a 422 gives for the identity field, without the field's name. */
function identityError(error: unknown): string | undefined {
  if (!(error instanceof ApiError) || error.status !== 422) return undefined;
  const line = error.problem?.errors?.[0] ?? "";
  return line.startsWith("identity:") ? line.slice("identity:".length).trim() : undefined;
}

function PlanTable({ plan }: { plan: DataPlan }): JSX.Element {
  const { t } = useTranslation();
  const rows = Object.entries(plan.modules).flatMap(([module, tables]) =>
    Object.entries(tables).map(([table, counts]) => ({ name: `${module}/${table}`, counts })),
  );
  const changed = rows.filter((row) => row.counts.create + row.counts.update + row.counts.refused > 0);
  const writes = rows.reduce((sum, row) => sum + row.counts.create + row.counts.update, 0);
  const refused = rows.reduce((sum, row) => sum + row.counts.refused, 0);
  return (
    <div className="flex flex-col gap-3">
      {changed.length > 0 ? <p className="text-body font-medium">{t("backups.data.planSummary", { writes, refused })}</p> : null}
      {changed.length === 0 ? (
        <Alert tone="success">{t("backups.data.planSame")}</Alert>
      ) : (
        <AdminGrid
          caption={t("backups.data.planCaption")}
          rows={changed}
          rowKey={(row) => row.name}
          storageKey="organization.backups.data.plan"
          empty={t("backups.data.planSame")}
          noSearch
          stack
          // A phone card shows the first three counts: refused comes before unchanged so it is never behind More details.
          columns={[
            { key: "table", label: t("backups.data.table"), text: (row) => row.name, cell: (row) => <span className="whitespace-nowrap font-mono">{row.name}</span> },
            { key: "create", label: t("backups.data.create"), kind: "number", align: "right", text: (row) => String(row.counts.create) },
            { key: "update", label: t("backups.data.update"), kind: "number", align: "right", text: (row) => String(row.counts.update) },
            { key: "refused", label: t("backups.data.refused"), kind: "number", align: "right", text: (row) => String(row.counts.refused) },
            { key: "unchanged", label: t("backups.data.unchanged"), kind: "number", align: "right", text: (row) => String(row.counts.unchanged) },
          ]}
        />
      )}
      {Object.keys(plan.notInArchive).length > 0 ? (
        <details>
          <summary className="cursor-pointer text-caption text-fg-muted">{t("backups.data.notInArchive", { count: Object.keys(plan.notInArchive).length })}</summary>
          <ul className="mt-1 list-disc pl-5 text-caption">
            {Object.entries(plan.notInArchive).map(([module, why]) => (
              <li key={module}>
                {module}: {why}
              </li>
            ))}
          </ul>
        </details>
      ) : null}
      <details>
        <summary className="cursor-pointer text-caption text-fg-muted">{t("backups.data.notRestored", { count: Object.keys(plan.notRestored).length })}</summary>
        <ul className="mt-1 list-disc pl-5 text-caption">
          {Object.entries(plan.notRestored).map(([table, why]) => (
            <li key={table}>
              {table}: {why}
            </li>
          ))}
        </ul>
      </details>
    </div>
  );
}

/** A data restore: the identity, the dry run, then the request a second administrator approves. */
function DataRestore({ version, onClose }: { version: string; onClose: () => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const client = useQueryClient();
  const [identity, setIdentity] = useState("");
  const call = useMutation({
    mutationFn: async (request: boolean): Promise<DataRestoreAnswer> =>
      (await doorAction("portal_backup_data_restore", {
        version,
        body: { identity: identity.trim(), request },
      })) as DataRestoreAnswer,
    onSuccess: async (answer) => {
      if (answer.request) setIdentity("");
      await client.invalidateQueries({ queryKey: overviewKey("data") });
      await client.invalidateQueries({ queryKey: RESTORES });
    },
  });
  const answer = call.data;
  const keyError = identityError(call.error);
  return (
    <Dialog
      open
      size="lg"
      onOpenChange={(open) => (open ? undefined : onClose())}
      title={t("backups.restore.title")}
      closeLabel={t("backups.restore.close")}
      description={answer ? t("backups.restore.taken", { at: when(answer.plan.takenAt, locale) }) : t("backups.data.restoreLead")}
    >
      <form
        className="flex flex-col gap-3"
        data-testid="backup-data-restore"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          call.mutate(false);
        }}
      >
        {answer?.request ? null : <IdentityField id="backup-data-identity" value={identity} onChange={setIdentity} error={keyError} />}
        {call.isError && !keyError ? <Alert tone="danger">{reasonOf(call.error, t("backups.unreadable"))}</Alert> : null}
        {answer ? <PlanTable plan={answer.plan} /> : null}
        {answer?.request ? <Alert tone="success">{t("backups.data.requested")}</Alert> : null}
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="secondary" type="button" onClick={onClose}>
            {t("backups.restore.close")}
          </Button>
          {answer?.request ? null : (
            <Button variant={answer ? "secondary" : "primary"} type="submit" disabled={call.isPending || identity.trim() === ""} disabledReason={identity.trim() === "" ? t("backups.data.identityFirst") : t("app.busyReason")}>
              {t("backups.data.check")}
            </Button>
          )}
          {answer && !answer.request && answer.plan.modules.portal ? (
            <Button variant="primary" type="button" onClick={() => call.mutate(true)} disabled={call.isPending || identity.trim() === ""} disabledReason={t("app.busyReason")}>
              {t("backups.data.request")}
            </Button>
          ) : null}
        </div>
      </form>
    </Dialog>
  );
}

/** The reason a 422 gives for `field`, without the field's name. */
function fieldReason(error: unknown, field: string): string | undefined {
  if (!(error instanceof ApiError) || error.status !== 422) return undefined;
  const line = (error.problem?.errors ?? []).find((l) => l.startsWith(`${field}:`));
  return line ? line.slice(field.length + 1).trim() : undefined;
}

/**
 * One project of a data backup restored into a new project (T-7208): the identity opens the
 * archive and lists the projects the server lets this administrator read, then a check, then
 * the copy. The project it came from is never written.
 */
function ProjectCopy({ version, onClose }: { version: string; onClose: () => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const client = useQueryClient();
  const [identity, setIdentity] = useState("");
  const [project, setProject] = useState("");
  const [name, setName] = useState("");
  const list = useMutation({
    mutationFn: async (): Promise<ArchivedProjects> =>
      (await doorAction("portal_backup_data_projects", { version, body: { identity: identity.trim() } })) as ArchivedProjects,
    onSuccess: (answer: ArchivedProjects) => pick(answer.projects.length === 1 ? answer.projects[0].id : "", answer),
  });
  const copy = useMutation({
    mutationFn: async (apply: boolean): Promise<CopyPlan> =>
      (await doorAction("portal_backup_data_copy", {
        version,
        body: { identity: identity.trim(), project, name: name.trim(), apply },
      })) as CopyPlan,
    onSuccess: async (plan) => {
      if (plan.projectId) {
        setIdentity("");
        await client.invalidateQueries({ queryKey: overviewKey("data") });
      }
    },
  });
  // A picked project suggests `<name>-restored` while the name is still empty or a suggestion.
  function pick(id: string, answer: ArchivedProjects | undefined = list.data): void {
    setProject(id);
    const source = answer?.projects.find((p) => p.id === id)?.name;
    setName((typed) => (typed === "" || typed.endsWith("-restored") ? (source ? `${source}-restored` : "") : typed));
    copy.reset();
  }
  const projects = list.data;
  const plan = copy.data;
  const done = Boolean(plan?.projectId);
  const keyError = identityError(list.error) ?? identityError(copy.error);
  const projectError = fieldReason(copy.error, "project");
  const nameError = fieldReason(copy.error, "name");
  const otherError = [list, copy].find((m) => m.isError && !identityError(m.error) && !fieldReason(m.error, "project") && !fieldReason(m.error, "name"));
  const ready = identity.trim() !== "" && project !== "" && name.trim() !== "";
  const rows = plan ? Object.entries(plan.tables).filter(([, c]) => c.create + c.refused > 0) : [];
  const total = (key: "create" | "refused") => rows.reduce((sum, [, c]) => sum + c[key], 0);
  return (
    <Dialog
      open
      size="lg"
      onOpenChange={(open) => (open ? undefined : onClose())}
      title={t("backups.copy.title")}
      closeLabel={t("backups.restore.close")}
      description={plan ? t("backups.restore.taken", { at: when(plan.takenAt, locale) }) : t("backups.copy.lead")}
    >
      <form
        className="flex flex-col gap-3"
        data-testid="backup-project-copy"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          if (projects) copy.mutate(false);
          else list.mutate();
        }}
      >
        {done ? null : <IdentityField id="backup-copy-identity" value={identity} onChange={setIdentity} error={keyError} />}
        {projects && !done ? (
          projects.projects.length === 0 ? (
            <EmptyState bare title={t("backups.copy.none")} />
          ) : (
            <>
              <Field id="backup-copy-project" label={t("backups.copy.project")} help={t("backups.copy.projectHint")} errors={projectError ? [projectError] : undefined}>
                <Select id="backup-copy-project" value={project} onChange={(e) => pick(e.target.value)}>
                  <option value="">{t("backups.copy.pick")}</option>
                  {projects.projects.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </Select>
              </Field>
              {projects.deleted.length > 0 ? (
                <p className="text-caption text-fg-muted">{t("backups.copy.deleted", { names: projects.deleted.join(", ") })}</p>
              ) : null}
              <Field id="backup-copy-name" label={t("backups.copy.name")} help={t("backups.copy.nameHint")} errors={nameError ? [nameError] : undefined}>
                <Input id="backup-copy-name" autoComplete="off" spellCheck={false} value={name}
                  onChange={(e) => {
                    setName(e.target.value);
                    copy.reset();
                  }} />
              </Field>
            </>
          )
        ) : null}
        {otherError ? <Alert tone="danger">{reasonOf(otherError.error, t("backups.unreadable"))}</Alert> : null}
        {plan && !done ? (
          <>
            <p className="text-body font-medium">
              {t("backups.copy.summary", { rows: total("create"), name: plan.name, refused: total("refused"), erasures: plan.erasuresReplayed })}
            </p>
            {rows.length > 0 ? (
              // Three short columns fit a phone as they are: no stacked cards, no column menu.
              <div className="overflow-x-auto">
                <table className="w-full text-body">
                  <caption className="sr-only">{t("backups.copy.caption")}</caption>
                  <thead>
                    <tr className="border-b border-border text-left text-caption text-fg-muted">
                      <th scope="col" className="py-1 pr-2 font-medium">{t("backups.data.table")}</th>
                      <th scope="col" className="py-1 pr-2 text-right font-medium">{t("backups.data.create")}</th>
                      <th scope="col" className="py-1 text-right font-medium">{t("backups.data.refused")}</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border">
                    {rows.map(([table, c]) => (
                      <tr key={table} className={c.refused > 0 ? "bg-warning-soft" : undefined}>
                        <th scope="row" className="py-1 pr-2 text-left font-mono font-normal break-all">{table}</th>
                        <td className="py-1 pr-2 text-right tabular-nums">{c.create}</td>
                        <td className="py-1 text-right tabular-nums">{c.refused}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ) : null}
            {total("refused") > 0 ? <Alert tone="warning">{t("backups.copy.refusedNote")}</Alert> : null}
          </>
        ) : null}
        {done && plan ? <Alert tone="success">{t("backups.copy.done", { name: plan.name, id: plan.projectId, source: plan.sourceName })}</Alert> : null}
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="secondary" type="button" onClick={onClose}>
            {t("backups.restore.close")}
          </Button>
          {done ? null : projects ? (
            <Button variant={plan ? "secondary" : "primary"} type="submit" disabled={copy.isPending || !ready} disabledReason={ready ? t("app.busyReason") : t("backups.copy.first")}>
              {t("backups.copy.check")}
            </Button>
          ) : (
            <Button variant="primary" type="submit" disabled={list.isPending || identity.trim() === ""} disabledReason={identity.trim() === "" ? t("backups.data.identityFirst") : t("app.busyReason")}>
              {t("backups.copy.list")}
            </Button>
          )}
          {plan && !done ? (
            <Button variant="primary" type="button" onClick={() => copy.mutate(true)} disabled={copy.isPending || !ready} disabledReason={t("app.busyReason")}>
              {t("backups.copy.apply")}
            </Button>
          ) : null}
        </div>
      </form>
    </Dialog>
  );
}

/** Restores waiting for a second administrator, and the ones decided. */
function RestoreRequests(): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const me = useIdentity()?.username;
  const requests = useQuery({
    queryKey: RESTORES,
    queryFn: async () => (await door("portal_backup_data_restores", {})) as RestoreRequest[],
  });
  const list = requests.data ?? [];
  if (requests.isLoading) return null;
  return (
    <Card>
      <CardHeader title={t("backups.data.requestsTitle")} description={t("backups.data.requestsLead")} />
      {requests.isError ? <Alert tone="danger">{reasonOf(requests.error, t("backups.unreadable"))}</Alert> : null}
      {list.length === 0 && !requests.isError ? <EmptyState bare title={t("backups.data.requestsEmpty")} description={t("backups.data.requestsEmptyHint")} /> : null}
      {list.length > 0 ? (
        <ul className="flex flex-col divide-y divide-border" aria-label={t("backups.data.requestsTitle")}>
          {list.map((request) => (
            <RestoreRow key={request.id} request={request} mine={request.requestedBy === me} locale={locale} />
          ))}
        </ul>
      ) : null}
    </Card>
  );
}

function RestoreRow({ request, mine, locale }: { request: RestoreRequest; mine: boolean; locale: string }): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [identity, setIdentity] = useState("");
  const settle = async () => {
    await client.invalidateQueries({ queryKey: RESTORES });
    await client.invalidateQueries({ queryKey: overviewKey("data") });
  };
  const approve = useMutation({
    mutationFn: async () =>
      doorAction("portal_backup_data_restore_approve", {
        id: String(request.id),
        body: { identity: identity.trim() },
        }),
    onSuccess: async () => {
      setIdentity("");
      await settle();
    },
  });
  const decline = useMutation({
    mutationFn: async () => doorAction("portal_backup_data_restore_decline", { id: String(request.id) }),
    onSuccess: settle,
  });
  const writes = Object.values(request.plan.modules)
    .flatMap((tables) => Object.values(tables))
    .reduce((sum, c) => sum + c.create + c.update, 0);
  const tone = request.state === "applied" ? "success" : request.state === "pending" ? "primary" : request.state === "declined" ? "neutral" : "danger";
  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={tone}>{t(`backups.data.state.${request.state}`)}</Badge>
        <span className="text-body">{t("backups.data.requestLine", { at: when(request.requestedAt, locale), by: request.requestedBy, rows: writes })}</span>
      </div>
      {request.decidedBy ? (
        <p className="text-caption text-fg-muted">
          {t("backups.data.decidedLine", { by: request.decidedBy, at: when(request.decidedAt, locale) })}
          {request.message ? ` ${request.message}` : ""}
        </p>
      ) : null}
      {request.state === "pending" && mine ? <p className="text-caption text-fg-muted">{t("backups.data.waitsForOther")}</p> : null}
      {request.state === "pending" && !mine ? (
        <form
          className="flex flex-col gap-2"
          noValidate
          onSubmit={(event) => {
            event.preventDefault();
            approve.mutate();
          }}
        >
          <IdentityField id={`backup-approve-${request.id}`} value={identity} onChange={setIdentity} error={identityError(approve.error)} />
          {approve.isError && !identityError(approve.error) ? <Alert tone="danger">{reasonOf(approve.error, t("backups.unreadable"))}</Alert> : null}
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" type="submit" disabled={approve.isPending || identity.trim() === ""} disabledReason={identity.trim() === "" ? t("backups.data.identityFirst") : t("app.busyReason")}>
              {t("backups.data.approve")}
            </Button>
            <Button variant="secondary" type="button" onClick={() => decline.mutate()} disabled={decline.isPending} disabledReason={t("app.busyReason")}>
              {t("backups.data.decline")}
            </Button>
          </div>
        </form>
      ) : null}
      {request.state === "pending" && mine ? (
        <div>
          <Button size="sm" variant="secondary" onClick={() => decline.mutate()} disabled={decline.isPending} disabledReason={t("app.busyReason")}>
            {t("backups.data.withdraw")}
          </Button>
        </div>
      ) : null}
      {decline.isError ? <Alert tone="danger">{reasonOf(decline.error, t("backups.unreadable"))}</Alert> : null}
    </li>
  );
}

function TargetForm({
  kind,
  target,
  fingerprints,
}: {
  kind: Kind;
  target: Target | null;
  fingerprints: string[];
}): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  // A new data target starts from the configuration backup's bucket, if one is saved: most
  // Organizations keep both in one bucket. The secret key is never prefilled.
  const [form, setForm] = useState<Form>(() =>
    formOf(target ?? (kind === "data" ? client.getQueryData<Overview>(overviewKey("configuration"))?.target : null)),
  );
  const save = useMutation({
    mutationFn: async () => {
      const body = {
        endpoint: form.endpoint.trim(),
        region: form.region.trim(),
        bucket: form.bucket.trim(),
        prefix: form.prefix.trim(),
        accessKeyId: form.accessKeyId.trim(),
        secretAccessKey: form.secretAccessKey === "" ? null : form.secretAccessKey,
        enabled: form.enabled,
        everyHours: Number(form.everyHours),
        keepDays: Number(form.keepDays),
      };
      return kind === "data"
        ? doorAction("portal_backup_data_set", {
            body: { ...body, workingRecipient: form.workingRecipient.trim(), recoveryRecipient: form.recoveryRecipient.trim() },
          })
        : doorAction("portal_backup_configuration_set", { body });
    },
    onSuccess: async () => {
      await client.invalidateQueries({ queryKey: overviewKey(kind) });
      await client.invalidateQueries({ queryKey: versionsKey(kind) });
    },
  });
  const invalid = fieldError(save.error);
  const set = (name: keyof Form) => (event: { target: { value: string } }) =>
    setForm((current) => ({ ...current, [name]: event.target.value }));
  const errorsOf = (name: FieldName) => (invalid?.field === name ? [invalid.reason] : undefined);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    save.mutate();
  };
  const text = (name: FieldName, label: string, help: string, type = "text") => (
    <Field id={`backup-${kind}-${name}`} label={label} help={help} errors={errorsOf(name)}>
      <Input
        id={`backup-${kind}-${name}`}
        type={type}
        value={form[name]}
        onChange={set(name)}
        autoComplete="off"
        placeholder={name === "secretAccessKey" && target ? t("backups.form.secretKept") : undefined}
      />
    </Field>
  );
  // An age public key is 62 characters: two lines show it whole on a phone.
  const recipient = (name: FieldName, label: string, help: string) => (
    <Field id={`backup-${kind}-${name}`} label={label} help={help} errors={errorsOf(name)}>
      <Textarea
        id={`backup-${kind}-${name}`}
        rows={2}
        className="font-mono"
        spellCheck={false}
        value={form[name]}
        onChange={set(name)}
        autoComplete="off"
      />
    </Field>
  );
  return (
    <Card>
      <CardHeader title={t("backups.form.title")} description={target ? t("backups.form.leadSaved") : t("backups.form.leadNew")} />
      {target || save.isError ? null : (
        <EmptyState bare title={t("backups.form.emptyTitle")} description={t("backups.form.emptyHint")} />
      )}
      <form onSubmit={submit} className="grid grid-cols-1 gap-4 md:grid-cols-2" noValidate>
        {text("endpoint", t("backups.form.endpoint"), t("backups.hint.endpoint"), "url")}
        {text("region", t("backups.form.region"), t("backups.hint.region"))}
        {text("bucket", t("backups.form.bucket"), t("backups.hint.bucket"))}
        {text("prefix", t("backups.form.prefix"), t("backups.hint.prefix"))}
        {text("accessKeyId", t("backups.form.accessKeyId"), t("backups.hint.accessKeyId"))}
        {text("secretAccessKey", t("backups.form.secretAccessKey"), t("backups.hint.secretAccessKey"), "password")}
        {text("everyHours", t("backups.form.everyHours"), t("backups.hint.everyHours"), "number")}
        {text("keepDays", t("backups.form.keepDays"), t("backups.hint.keepDays"), "number")}
        {kind === "data" ? (
          <>
            {recipient("workingRecipient", t("backups.form.workingRecipient"), t("backups.hint.workingRecipient"))}
            {recipient("recoveryRecipient", t("backups.form.recoveryRecipient"), t("backups.hint.recoveryRecipient"))}
            {fingerprints.length === 2 ? (
              <dl className="md:col-span-2 grid grid-cols-1 gap-3 text-caption sm:grid-cols-2" data-testid="backup-fingerprints">
                {fingerprints.map((print, i) => (
                  <div key={print} className="flex flex-col">
                    <dt className="text-fg-muted">
                      {i === 0 ? t("backups.data.workingFingerprint") : t("backups.data.recoveryFingerprint")}
                    </dt>
                    <dd className="font-mono break-all">{print}</dd>
                  </div>
                ))}
              </dl>
            ) : null}
          </>
        ) : null}
        <div className="md:col-span-2">
          <Switch
            checked={form.enabled}
            onCheckedChange={(enabled) => setForm((current) => ({ ...current, enabled }))}
            label={t("backups.form.enabled")}
            id={`backup-${kind}-enabled`}
          />
          <p className="mt-1 text-caption text-fg-muted">{t("backups.hint.enabled")}</p>
        </div>
        {invalid ? (
          <Alert tone="danger" className="md:col-span-2">
            {t("backups.form.invalid", { field: t(`backups.form.${invalid.field}`) })}
          </Alert>
        ) : null}
        {save.isError && !invalid ? (
          <Alert tone="danger" className="md:col-span-2">
            {reasonOf(save.error, t("backups.form.failed"))}
          </Alert>
        ) : null}
        {save.isSuccess ? (
          <Alert tone="success" className="md:col-span-2">
            {t("backups.form.saved")}
          </Alert>
        ) : null}
        <div className="md:col-span-2">
          <Button variant="primary" type="submit" disabled={save.isPending} disabledReason={t("app.busyReason")}>
            {save.isPending ? t("backups.form.saving") : t("backups.form.save")}
          </Button>
        </div>
      </form>
    </Card>
  );
}
