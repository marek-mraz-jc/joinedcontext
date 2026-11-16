import { useState } from "react";
import type { FormEvent, JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError, api, reasonOf, unwrap } from "../../api/client";
import { useAdministers } from "../../api/permissions";
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
  Switch,
} from "../../components/ui";

type Overview = components["schemas"]["BackupOverview"];
type Target = components["schemas"]["Target"];
type VersionList = components["schemas"]["VersionList"];
type RestorePlan = components["schemas"]["RestorePlan"];

const DOOR = "/api/v1/organization/backups/configuration";
const OVERVIEW = ["organizationBackup"] as const;
const VERSIONS = ["organizationBackup", "versions"] as const;
/** The restore plan group of the bundle's people (T-7164): listed, never created. */
const PEOPLE = "organization/people";

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
  const overview = useQuery({
    queryKey: OVERVIEW,
    enabled: allowed,
    queryFn: async (): Promise<Overview> => unwrap(await api.GET(DOOR)),
  });
  if (!known) return <p className="text-body text-fg-muted">{t("backups.loading")}</p>;
  if (!allowed) return <Alert tone="info">{t("organization.adminOnly")}</Alert>;
  if (overview.isLoading) return <p className="text-body text-fg-muted">{t("backups.loading")}</p>;
  if (overview.isError) return <Alert tone="danger">{reasonOf(overview.error, t("backups.unreadable"))}</Alert>;
  const target = overview.data?.target ?? null;
  return (
    <div className="flex flex-col gap-6" data-testid="organization-backups">
      <p className="text-body text-fg-muted">{t("backups.lead")}</p>
      {target ? <Runs target={target} overview={overview.data} /> : null}
      {target ? <Versions /> : null}
      <TargetForm key={target?.updatedAt ?? "new"} target={target} />
    </div>
  );
}

function Runs({ target, overview }: { target: Target; overview: Overview | undefined }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const client = useQueryClient();
  const run = useMutation({
    mutationFn: async () => unwrap(await api.POST(`${DOOR}/run`)),
    onSettled: async () => client.invalidateQueries({ queryKey: OVERVIEW }),
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

function Versions(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const client = useQueryClient();
  const [deleting, setDeleting] = useState(false);
  const [restoring, setRestoring] = useState<string | null>(null);
  const versions = useQuery({
    queryKey: VERSIONS,
    queryFn: async (): Promise<VersionList> => unwrap(await api.GET(`${DOOR}/versions`)),
  });
  const remove = useMutation({
    mutationFn: async () => unwrap(await api.DELETE(`${DOOR}/bundle`)),
    onSettled: async () => {
      setDeleting(false);
      await client.invalidateQueries({ queryKey: OVERVIEW });
      await client.invalidateQueries({ queryKey: VERSIONS });
    },
  });
  const save = async (version: string) => {
    const answer = await api.GET(`${DOOR}/versions/{version}`, {
      params: { path: { version } },
      parseAs: "blob",
    });
    const blob = await unwrap(answer as { data?: Blob; error?: unknown; response: Response });
    const href = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = href;
    link.download = `configuration-${version}.zip`;
    link.click();
    URL.revokeObjectURL(href);
  };
  const list = versions.data?.versions ?? [];
  const current = list.find((version) => version.isLatest);
  return (
    <Card>
      <CardHeader
        title={t("backups.versions.title")}
        description={t("backups.versions.lead")}
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
      {restoring ? <Restore version={restoring} onClose={() => setRestoring(null)} /> : null}
    </Card>
  );
}

function Restore({ version, onClose }: { version: string; onClose: () => void }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en";
  const client = useQueryClient();
  const call = async (apply: boolean): Promise<RestorePlan> =>
    unwrap(
      await api.POST(`${DOOR}/versions/{version}/restore`, {
        params: { path: { version } },
        body: { apply },
      }),
    );
  const plan = useQuery({ queryKey: ["organizationBackup", "plan", version], queryFn: () => call(false) });
  const apply = useMutation({
    mutationFn: () => call(true),
    onSettled: async () => client.invalidateQueries({ queryKey: OVERVIEW }),
  });
  const shown = apply.data ?? plan.data;
  const projectChanges = (shown?.groups ?? []).filter(
    (group) => group.name !== "organization" && group.name !== PEOPLE && group.created.length + group.changed.length > 0,
  );
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
        {shown && shown.changes === 0 ? <Alert tone="success">{t("backups.restore.same")}</Alert> : null}
        {shown?.groups.map((group) => (
          <section key={group.name} className="flex flex-col gap-1">
            <h3 className="text-body font-semibold">
              {group.name === "organization"
                ? t("backups.restore.organization")
                : group.name === PEOPLE
                  ? t("backups.restore.people")
                  : t("backups.restore.project", { project: group.name })}
            </h3>
            <p className="text-caption text-fg-muted">
              {t("backups.restore.counts", {
                created: group.created.length,
                changed: group.changed.length,
                unchanged: group.unchanged,
              })}
            </p>
            {[...group.created, ...group.changed].length > 0 ? (
              <ul className="list-disc pl-5 text-caption">
                {[...group.created, ...group.changed].map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            ) : null}
            {group.note ? (
              <p className="text-caption text-fg-muted">
                {t(group.name === PEOPLE ? "backups.restore.peopleNote" : "backups.restore.organizationNote")}
              </p>
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

function TargetForm({ target }: { target: Target | null }): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [form, setForm] = useState<Form>(() => formOf(target));
  const save = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.PUT(DOOR, {
          body: {
            endpoint: form.endpoint.trim(),
            region: form.region.trim(),
            bucket: form.bucket.trim(),
            prefix: form.prefix.trim(),
            accessKeyId: form.accessKeyId.trim(),
            secretAccessKey: form.secretAccessKey === "" ? null : form.secretAccessKey,
            enabled: form.enabled,
            everyHours: Number(form.everyHours),
            keepDays: Number(form.keepDays),
          },
        }),
      ),
    onSuccess: async () => {
      await client.invalidateQueries({ queryKey: OVERVIEW });
      await client.invalidateQueries({ queryKey: VERSIONS });
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
    <Field id={`backup-${name}`} label={label} help={help} errors={errorsOf(name)}>
      <Input
        id={`backup-${name}`}
        type={type}
        value={form[name]}
        onChange={set(name)}
        autoComplete="off"
        placeholder={name === "secretAccessKey" && target ? t("backups.form.secretKept") : undefined}
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
        <div className="md:col-span-2">
          <Switch
            checked={form.enabled}
            onCheckedChange={(enabled) => setForm((current) => ({ ...current, enabled }))}
            label={t("backups.form.enabled")}
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
