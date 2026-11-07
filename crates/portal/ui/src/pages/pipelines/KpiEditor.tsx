/**
 * The KPI editor over several sources (PL-105, T-4935): two to five Endpoints of the project, how
 * their rows meet, a formula from plain choices (the typed one only under Advanced), each field
 * checked by jc-core's rules as it is typed, a preview on the sources' current pages, then
 * Propose, which writes the `v1alpha2` Pipeline through the change flow like every other write.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { usePermissions } from "../../api/permissions";
import { useProjectOrganization } from "../../api/projects";
import { useBranding } from "../../branding";
import type { components } from "../../api/schema";
import { AttributePicker } from "../../components/pickers/AttributePicker";
import { useDebounced } from "../../components/pickers/organizationModels";
import { TypePicker } from "../../components/pickers/TypePicker";
import { Alert, Badge, Button, Dialog, Field, Input, Select, Switch, Textarea } from "../../components/ui";
import { Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../../components/ui/Table";
import { spaceOf } from "../spaces/spaceFacts";
import { reachableEndpoints } from "./PipelineEditor";
import {
  FORMULA_CHOICES,
  MAX_SOURCES,
  PREVIEW_PAGE,
  SCHEDULES,
  emptyDraft,
  emptySource,
  formulaOf,
  kpiManifest,
  kpiTargets,
  leastSources,
  missingOf,
  placeOf,
  withChoice,
} from "./kpiEditor";
import type { FormulaChoice, JoinBy, KpiDraft, KpiSourceDraft, ProblemPlace, ScheduleChoice } from "./kpiEditor";

type Checked = components["schemas"]["KpiChecked"];
type Problem = components["schemas"]["KpiProblem"];

export interface KpiEditorProps {
  project: string;
  /** What the editor starts with; blank when absent. */
  initial?: KpiDraft;
  pending: boolean;
  error: string | null;
  onClose: () => void;
  onPropose: (manifest: Manifest) => void;
}

function useKpiCheck(project: string, manifest: Manifest, ready: boolean) {
  const body = useDebounced(JSON.stringify(manifest), 400);
  return useQuery({
    queryKey: ["kpi-check", project, body],
    enabled: ready,
    queryFn: async (): Promise<Checked> =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/kpis/check", {
          params: { path: { project } },
          body: { pipeline: JSON.parse(body) as { [key: string]: unknown }, preview: false },
        }),
      ),
  });
}

function problemsAt(problems: Problem[], match: (place: ProblemPlace) => boolean): string[] | undefined {
  const found = problems.filter((p) => match(placeOf(p.field))).map((p) => p.reason);
  return found.length ? found : undefined;
}

/** The project's Endpoints the person may read, as the pipeline form lists them. */
function useReadableEndpoints(project: string): Manifest[] {
  const permissions = usePermissions(project);
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });
  return useMemo(
    () => reachableEndpoints(permissions.data, asManifests(endpoints.data?.items ?? [])),
    [endpoints.data, permissions.data],
  );
}

export function KpiEditorDialog({ project, initial, pending, error, onClose, onPropose }: KpiEditorProps): JSX.Element {
  const { t } = useTranslation();
  const branding = useBranding();
  // The project's own Organization's domain; none at all until it is known, so no target URN is
  // minted under a stand-in and fixed into the draft (F-UI3-016).
  const { organization, settled } = useProjectOrganization(project);
  const orgDomain = (organization?.spec as { domain?: string } | undefined)?.domain || (settled ? branding.orgDomain : "");
  const endpoints = useReadableEndpoints(project);
  const targets = useMemo(() => kpiTargets(endpoints, spaceOf, orgDomain), [endpoints, orgDomain]);
  const sourceEndpoints = useMemo(() => endpoints.filter((e) => !spaceOf(e)?.endsWith("-kpi")), [endpoints]);
  const [draft, setDraft] = useState<KpiDraft>(() => initial ?? emptyDraft());
  // The only KPI target is the default once the Endpoints arrive.
  const target = draft.target || (targets.length === 1 ? (targets[0]?.urn ?? "") : "");
  if (target !== draft.target) setDraft((d) => ({ ...d, target }));
  const manifest = useMemo(() => kpiManifest(project, draft), [project, draft]);
  const missing = missingOf(draft);
  const check = useKpiCheck(project, manifest, !missing);
  const problems = check.data?.problems ?? [];
  const at = (place: ProblemPlace["at"]) => problemsAt(problems, (p) => p.at === place);
  const update = (patch: Partial<KpiDraft>) => setDraft((d) => ({ ...d, ...patch }));
  const preview = useMutation({
    mutationFn: async (): Promise<Checked> =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/kpis/check", {
          params: { path: { project } },
          body: { pipeline: manifest as unknown as { [key: string]: unknown }, preview: true },
        }),
      ),
  });
  const checkFailed = check.error instanceof ApiError ? (check.error.problem?.detail ?? check.error.message) : undefined;
  const clean = !missing && check.isSuccess && problems.length === 0 && !check.isFetching;
  const proposeReason = missing
    ? t(`pipelines.kpiEditor.missing.${missing}`)
    : clean
      ? undefined
      : t("pipelines.kpiEditor.fixFirst");

  return (
    <Dialog
      open
      onOpenChange={(open) => (open ? undefined : onClose())}
      title={t("pipelines.kpiEditor.title")}
      description={t("pipelines.kpiEditor.lead")}
      size="lg"
      closeLabel={t("app.close")}
      footer={
        <div className="flex flex-wrap items-center justify-end gap-2">
          {proposeReason ? <span className="text-caption text-fg-muted" id="kpi-propose-reason">{proposeReason}</span> : null}
          <Button variant="secondary" onClick={onClose}>
            {t("app.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={!!proposeReason || pending}
            aria-describedby={proposeReason ? "kpi-propose-reason" : undefined}
            onClick={() => onPropose(manifest)}
          >
            {t("pipelines.propose")}
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-4" data-testid="kpi-editor">
        {error ? <Alert tone="danger">{error}</Alert> : null}
        {checkFailed ? <Alert tone="danger">{checkFailed}</Alert> : null}
        {at("general") ? <Alert tone="danger">{at("general")?.join(" ")}</Alert> : null}
        <div className="grid gap-3 sm:grid-cols-2">
          <Field id="kpi-name" label={t("pipelines.kpiEditor.name")} help={t("pipelines.kpiEditor.hint.name")} errors={at("name")} required>
            <Input id="kpi-name" value={draft.name} onChange={(e) => update({ name: e.target.value.trim() })} placeholder="parking-per-stop" />
          </Field>
          <Field id="kpi-title" label={t("pipelines.kpiEditor.titleField")} help={t("pipelines.kpiEditor.hint.title")}>
            <Input id="kpi-title" value={draft.title} onChange={(e) => update({ title: e.target.value })} />
          </Field>
        </div>
        <SourceList project={project} draft={draft} endpoints={sourceEndpoints} problems={problems} checked={check.data} onChange={(sources) => update({ sources })} />
        <JoinFields draft={draft} problems={at("join")} update={update} />
        <FormulaFields draft={draft} problems={at("formula")} update={update} />
        <div className="grid gap-3 sm:grid-cols-3">
          <Field id="kpi-unit" label={t("pipelines.kpiEditor.unit")} help={t("pipelines.kpiEditor.hint.unit")} errors={at("unit")} required>
            <Input id="kpi-unit" value={draft.unitCode} onChange={(e) => update({ unitCode: e.target.value.toUpperCase() })} />
          </Field>
          <Field id="kpi-schedule" label={t("pipelines.kpiEditor.schedule")} help={t("pipelines.kpiEditor.hint.schedule")} errors={at("schedule")} required>
            <Select id="kpi-schedule" value={draft.schedule} onChange={(e) => update({ schedule: e.target.value as ScheduleChoice })}>
              {(Object.keys(SCHEDULES) as ScheduleChoice[]).map((one) => (
                <option key={one} value={one}>
                  {t(`pipelines.kpiEditor.every.${one}`)}
                </option>
              ))}
            </Select>
          </Field>
          <Field
            id="kpi-target"
            label={t("pipelines.kpiEditor.target")}
            help={targets.length ? t("pipelines.kpiEditor.hint.target") : t("pipelines.kpiEditor.noTarget")}
            errors={at("target")}
            required
          >
            <Select id="kpi-target" value={draft.target} onChange={(e) => update({ target: e.target.value })}>
              <option value="">{t("pipelines.recipe.choose")}</option>
              {targets.map((one) => (
                <option key={one.urn} value={one.urn}>
                  {one.name}
                </option>
              ))}
            </Select>
          </Field>
        </div>
        <PreviewPanel
          disabled={!clean}
          running={preview.isPending}
          answer={preview.data}
          failed={preview.error instanceof Error ? (preview.error instanceof ApiError ? (preview.error.problem?.detail ?? preview.error.message) : t("app.error.generic")) : undefined}
          onRun={() => preview.mutate()}
          hint={!draft.advanced && draft.choice === "change" ? t("pipelines.kpiEditor.previewHintChange") : t("pipelines.kpiEditor.previewHint", { formula: formulaOf(draft) })}
        />
      </div>
    </Dialog>
  );
}

interface SourceListProps {
  project: string;
  draft: KpiDraft;
  endpoints: Manifest[];
  problems: Problem[];
  checked?: Checked;
  onChange: (sources: KpiSourceDraft[]) => void;
}

function SourceList({ project, draft, endpoints, problems, checked, onChange }: SourceListProps): JSX.Element {
  const { t } = useTranslation();
  const set = (index: number, patch: Partial<KpiSourceDraft>) =>
    onChange(draft.sources.map((s, i) => (i === index ? { ...s, ...patch } : s)));
  return (
    <fieldset className="flex flex-col gap-3 rounded-md border border-border p-3">
      <legend className="px-1 text-body font-semibold text-fg">{t("pipelines.kpiEditor.sources")}</legend>
      <p className="text-caption text-fg-muted">{t("pipelines.kpiEditor.hint.sources")}</p>
      {endpoints.length === 0 ? <Alert tone="info">{t("pipelines.kpiEditor.noEndpoints")}</Alert> : null}
      {draft.sources.map((source, index) => (
        <SourceRow
          key={index}
          project={project}
          index={index}
          source={source}
          join={draft.join}
          endpoints={endpoints}
          errors={problemsAt(problems, (p) => p.at === "source" && p.index === index)}
          readable={checked?.sources.find((s) => s.name === source.name)?.readable}
          canRemove={draft.sources.length > leastSources(draft)}
          onChange={(patch) => set(index, patch)}
          onRemove={() => onChange(draft.sources.filter((_, i) => i !== index))}
        />
      ))}
      {draft.sources.length < MAX_SOURCES ? (
        <div>
          <Button variant="secondary" size="sm" onClick={() => onChange([...draft.sources, emptySource(draft.sources.length)])}>
            {t("pipelines.kpiEditor.addSource")}
          </Button>
        </div>
      ) : null}
    </fieldset>
  );
}

interface SourceRowProps {
  project: string;
  index: number;
  source: KpiSourceDraft;
  join: JoinBy;
  endpoints: Manifest[];
  errors?: string[];
  readable?: boolean;
  canRemove: boolean;
  onChange: (patch: Partial<KpiSourceDraft>) => void;
  onRemove: () => void;
}

function SourceRow({ project, index, source, join, endpoints, errors, readable, canRemove, onChange, onRemove }: SourceRowProps): JSX.Element {
  const { t } = useTranslation();
  const id = `kpi-source-${index}`;
  const space = spaceOf(endpoints.find((e) => e.metadata.name === source.endpoint) ?? ({ metadata: { name: "" }, spec: {} } as unknown as Manifest));
  // The first source carries the hints; a second copy of each only lengthens the form.
  const hint = (key: string) => (index === 0 ? t(key) : undefined);
  return (
    <div className="grid gap-2 rounded-md bg-surface-subtle p-2 sm:grid-cols-2" data-testid={id}>
      <Field id={`${id}-name`} label={t("pipelines.kpiEditor.sourceName")} help={hint("pipelines.kpiEditor.hint.sourceName")} errors={errors} required>
        <Input id={`${id}-name`} value={source.name} onChange={(e) => onChange({ name: e.target.value.trim().toLowerCase() })} />
      </Field>
      <Field id={`${id}-endpoint`} label={t("pipelines.kpiEditor.endpoint")} help={hint("pipelines.kpiEditor.hint.endpoint")} required>
        <Select id={`${id}-endpoint`} value={source.endpoint} onChange={(e) => onChange({ endpoint: e.target.value, type: "", attribute: "", key: "" })}>
          <option value="">{t("pipelines.recipe.choose")}</option>
          {endpoints.map((one) => (
            <option key={one.metadata.name} value={one.metadata.name}>
              {one.metadata.name}
            </option>
          ))}
        </Select>
      </Field>
      <Field id={`${id}-type`} label={t("pipelines.kpiEditor.type")} help={hint("pipelines.kpiEditor.hint.type")} required>
        <TypePicker
          id={`${id}-type`}
          label={t("pipelines.kpiEditor.type")}
          labelled
          project={project}
          space={space}
          value={source.type ? [source.type] : []}
          onChange={(types) => onChange({ type: types[0] ?? "", attribute: "", key: "" })}
        />
      </Field>
      <Field id={`${id}-attribute`} label={t("pipelines.kpiEditor.attribute")} help={source.type ? hint("pipelines.kpiEditor.hint.attribute") : t("pipelines.kpiEditor.hint.typeFirst")}>
        <AttributePicker
          id={`${id}-attribute`}
          label={t("pipelines.kpiEditor.attribute")}
          labelled
          project={project}
          space={space}
          type={source.type || undefined}
          kind="Property"
          numeric
          value={source.attribute ? [source.attribute] : []}
          onChange={(names) => onChange({ attribute: names[0] ?? "" })}
        />
      </Field>
      {join === "time" ? null : (
        <Field id={`${id}-key`} label={t(`pipelines.kpiEditor.key.${join}`)} help={t(`pipelines.kpiEditor.hint.${join}Key`)} required>
          <AttributePicker
            id={`${id}-key`}
            label={t(`pipelines.kpiEditor.key.${join}`)}
            labelled
            project={project}
            space={space}
            type={source.type || undefined}
            kind="any"
            value={source.key ? [source.key] : []}
            onChange={(names) => onChange({ key: names[0] ?? "" })}
          />
        </Field>
      )}
      <div className="flex flex-wrap items-center gap-2 sm:col-span-2">
        {readable === false ? (
          <Badge tone="warning">{t("pipelines.kpiEditor.notReadable")}</Badge>
        ) : readable ? (
          <Badge tone="success">{t("pipelines.kpiEditor.readable")}</Badge>
        ) : null}
        {readable === false ? <span className="text-caption text-fg-muted">{t("pipelines.kpiEditor.askAccess")}</span> : null}
        {canRemove ? (
          <Button variant="ghost" size="sm" onClick={onRemove} aria-label={t("pipelines.kpiEditor.removeSource", { name: source.name })}>
            {t("pipelines.kpiEditor.remove")}
          </Button>
        ) : null}
      </div>
    </div>
  );
}

function JoinFields({ draft, problems, update }: { draft: KpiDraft; problems?: string[]; update: (patch: Partial<KpiDraft>) => void }): JSX.Element {
  const { t } = useTranslation();
  // A change is computed over all rows of the run (PL-117).
  const fixed = !draft.advanced && draft.choice === "change";
  return (
    <Field
      id="kpi-join"
      label={t("pipelines.kpiEditor.join")}
      help={t(fixed ? "pipelines.kpiEditor.joinHint.change" : `pipelines.kpiEditor.joinHint.${draft.join}`)}
      errors={problems}
      required
    >
      <Select id="kpi-join" value={draft.join} disabled={fixed} title={fixed ? t("pipelines.kpiEditor.joinHint.change") : undefined} onChange={(e) => update({ join: e.target.value as JoinBy })}>
        {(["time", "area", "entity"] as JoinBy[]).map((one) => (
          <option key={one} value={one}>
            {t(`pipelines.kpiEditor.joinBy.${one}`)}
          </option>
        ))}
      </Select>
    </Field>
  );
}

interface FormulaFieldsProps {
  draft: KpiDraft;
  problems?: string[];
  update: (patch: Partial<KpiDraft>) => void;
}

function FormulaFields({ draft, problems, update }: FormulaFieldsProps): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2">
      {draft.advanced ? (
        <Field id="kpi-formula" label={t("pipelines.kpiEditor.formula")} help={t("pipelines.kpiEditor.hint.formula")} errors={problems} required>
          <Textarea id="kpi-formula" rows={2} value={draft.typed} onChange={(e) => update({ typed: e.target.value })} className="font-mono" />
        </Field>
      ) : (
        <Field
          id="kpi-choice"
          label={t("pipelines.kpiEditor.choice")}
          help={draft.choice === "change" ? t("pipelines.kpiEditor.hint.change") : t("pipelines.kpiEditor.writes", { formula: formulaOf(draft) })}
          errors={problems}
          required
        >
          <Select id="kpi-choice" value={draft.choice} onChange={(e) => update(withChoice(draft, e.target.value as FormulaChoice))}>
            {FORMULA_CHOICES.map((one) => (
              <option key={one} value={one}>
                {t(`pipelines.kpiEditor.choices.${one}`)}
              </option>
            ))}
          </Select>
        </Field>
      )}
      <Switch
        id="kpi-advanced"
        checked={draft.advanced}
        label={t("pipelines.kpiEditor.advanced")}
        onCheckedChange={(advanced) => update({ advanced, typed: draft.typed || formulaOf(draft) })}
      />
    </div>
  );
}

/** A source's row count; a read back goes by `parking@P7D` (PL-117). */
function rowsLine(t: TFunction, page: string, count: number): string {
  const [name, period] = page.split("@");
  if (period === "P7D") return t("pipelines.kpiEditor.rowsWeekBack", { name, count });
  return period ? t("pipelines.kpiEditor.rowsBack", { name, period, count }) : t("pipelines.kpiEditor.rows", { name, count });
}

interface PreviewPanelProps {
  disabled: boolean;
  running: boolean;
  answer?: Checked;
  failed?: string;
  hint: string;
  onRun: () => void;
}

function PreviewPanel({ disabled, running, answer, failed, hint, onRun }: PreviewPanelProps): JSX.Element {
  const { t } = useTranslation();
  const preview = answer?.preview;
  const number = new Intl.NumberFormat(undefined, { maximumFractionDigits: 3 });
  // The answer lands at the foot of the scrolling dialog body.
  const end = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (answer || failed) end.current?.scrollIntoView?.({ block: "nearest" });
  }, [answer, failed]);
  return (
    <section className="flex flex-col gap-2 rounded-md border border-border p-3" aria-labelledby="kpi-preview-title" aria-live="polite">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 id="kpi-preview-title" className="text-body font-semibold text-fg">
          {t("pipelines.kpiEditor.preview")}
        </h3>
        <Button
          variant="secondary"
          size="sm"
          disabled={disabled || running}
          title={disabled ? t("pipelines.kpiEditor.previewFirst") : undefined}
          onClick={onRun}
        >
          {running ? t("pipelines.kpiEditor.previewing") : t("pipelines.kpiEditor.runPreview")}
        </Button>
      </div>
      <p className="text-caption text-fg-muted">{hint}</p>
      {!answer && !running && !failed && !disabled ? <p className="text-caption text-fg-muted">{t("pipelines.kpiEditor.noPreviewYet")}</p> : null}
      {failed ? <Alert tone="danger">{failed}</Alert> : null}
      {preview?.problem ? <Alert tone="warning">{preview.problem}</Alert> : null}
      {preview && !preview.problem && preview.values.length === 0 ? <p className="text-caption text-fg-muted">{t("pipelines.kpiEditor.noKeys")}</p> : null}
      {preview && preview.values.length > 0 ? (
        <Table caption={t("pipelines.kpiEditor.preview")}>
          <TableHead>
            <TableHeaderCell>{t("pipelines.kpiEditor.key.column")}</TableHeaderCell>
            <TableHeaderCell>{t("pipelines.kpiEditor.indicator")}</TableHeaderCell>
            <TableHeaderCell className="text-right">{t("pipelines.kpiEditor.value")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {preview.values.map((one) => (
              <TableRow key={one.name}>
                <TableCell>{one.key ?? t("pipelines.kpiEditor.wholeRun")}</TableCell>
                <TableCell className="font-mono text-caption">{one.name}</TableCell>
                <TableCell className="text-right tabular-nums">{number.format(one.value)}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}
      {preview ? (
        <p className="text-caption text-fg-muted">
          {Object.entries(preview.rows)
            .map(([page, rows]) => rowsLine(t, page, rows))
            .join(" · ")}
        </p>
      ) : null}
      {preview && Object.values(preview.rows).some((rows) => rows >= PREVIEW_PAGE) ? (
        <Alert tone="info">{t("pipelines.kpiEditor.partial", { rows: PREVIEW_PAGE })}</Alert>
      ) : null}
      <span ref={end} data-testid="kpi-preview-end" />
    </section>
  );
}
