/**
 * "Add field" on a space's data (T-3098, ADR-N-042 §3.1): a field is a slot of the type's LinkML
 * DataModel, so the dialog proposes a DataModel Change, reviewed like any other, and never changes
 * the data. The official Smart Data Model of the same name, when there is one, is offered first.
 */
import { useId, useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { api, unwrap } from "../../api/client";
import { importChecked } from "../../api/proposal";
import { writeModelSource } from "../../api/datamodelSource";
import { isChange } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { parseModel } from "../../pages/models/linkml";
import type { LinkmlModel, LinkmlSlot } from "../../pages/models/linkml";
import { applyOperations } from "../../pages/models/operations";
import type { Applied } from "../../pages/models/operations";
import type { Catalogue } from "../../pages/models/SmartDataModelsImport";
import type { Artifacts } from "../../pages/models/LinkmlPreviewPanel";
import { Alert, Button, Checkbox, Dialog, Field, Input, Select, Textarea } from "../ui";
import { EMPTY_DRAFT, FIELD_TYPES, fieldOperations, formulaProblem, formulaSlots, problemsOf, suggestedSlots, withFormula, withOfficialSlot } from "./fieldTypes";
import { checkFormulas, formulaManifests } from "./formula";
import { policyRefsOf } from "../endpoints/policyBinding";
import type { FieldDraft, FieldType } from "./fieldTypes";

export interface AddFieldDialogProps {
  project: string;
  /** The DataModel the type belongs to, by its manifest name. */
  modelName: string;
  /** Its committed LinkML source, which the Change edits. */
  source: string;
  /** The entity type, a class of the model. */
  type: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called with each Change proposed: the model's, and a formula field's pipeline (DM-80). */
  onProposed: (change: Change) => void;
  /** The space the type is read in, its Endpoints and the organization's domain: where a formula
   * field's pipeline reads and writes (DM-80). */
  space?: string;
  endpoints?: Manifest[];
  orgDomain?: string;
}

/** The official model's class of the same name, or nothing when the catalogue has none. */
async function officialModel(type: string): Promise<LinkmlModel | null> {
  const catalogue = (await unwrap(await api.GET("/api/v1/tools/sdm-catalog", {}))) as Catalogue;
  const entry = (catalogue.subjects ?? []).flatMap((s) => s.models ?? []).find((m) => m.name === type);
  if (!entry) return null;
  const artifacts = (await unwrap(await api.POST("/api/v1/tools/import-sdm", { body: { model: entry.id } }))) as Artifacts;
  return artifacts.linkml ? parseModel(artifacts.linkml) : null;
}

export function AddFieldDialog({ project, modelName, source, type, open, onOpenChange, onProposed, space, endpoints = [], orgDomain }: AddFieldDialogProps): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const model = useMemo(() => parseModel(source), [source]);
  const [draft, setDraft] = useState<FieldDraft>(EMPTY_DRAFT);
  const [valuesText, setValuesText] = useState("");
  const [tried, setTried] = useState(false);
  const [busy, setBusy] = useState(false);
  const [refusals, setRefusals] = useState<string[]>([]);

  const official = useQuery({
    queryKey: ["tools", "sdm-official", type],
    enabled: open,
    retry: false,
    staleTime: Infinity,
    queryFn: () => officialModel(type),
  });
  const suggestions = useMemo(
    () => (official.data ? suggestedSlots(official.data, model, type) : []),
    [official.data, model, type],
  );

  const problems = problemsOf(draft, model, type);
  const shown = tried ? problems : {};
  const set = (patch: Partial<FieldDraft>) => setDraft((before) => ({ ...before, ...patch }));

  function close(next: boolean) {
    if (!next) {
      setDraft(EMPTY_DRAFT);
      setValuesText("");
      setTried(false);
      setRefusals([]);
    }
    onOpenChange(next);
  }

  async function propose(applied: Applied) {
    setRefusals([]);
    if (applied.refused.length > 0) {
      setRefusals(applied.refused.map((r) => r.reason));
      return;
    }
    setBusy(true);
    try {
      const answer = await writeModelSource({ project, name: modelName, source: applied.source, dryRun: false });
      if (answer.kind === "proposed") {
        onProposed(answer.change);
        close(false);
      } else if (answer.kind === "refused") {
        setRefusals([answer.problem.detail ?? answer.problem.title ?? t("spaces.fields.refusedStatus", { status: answer.status })]);
      }
    } catch (error) {
      setRefusals([error instanceof Error ? error.message : String(error)]);
    } finally {
      setBusy(false);
    }
  }

  /**
   * A formula field is two Changes (DM-80): the slot with its `equals_expression`, then the class's
   * pipeline and its two Policies, compiled over every formula of the class. Neither is proposed
   * when the project cannot run the pipeline: no `pipelines` ServiceAccount, or no Endpoint of the
   * space that answers by every Policy of the space (one bound by `policyRefs` would ignore its new
   * Policies).
   */
  async function proposeFormula() {
    setRefusals([]);
    const through = endpoints.find((endpoint) => policyRefsOf(endpoint) === undefined);
    if (!space || !orgDomain || !through) {
      setRefusals([t("spaces.fields.formula.noEndpoint")]);
      return;
    }
    setBusy(true);
    try {
      const account = await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
        params: { path: { project, plural: "serviceaccounts", name: "pipelines" } },
      });
      if (account.response.status === 404) {
        setRefusals([t("spaces.fields.formula.noAccount")]);
        return;
      }
      await unwrap(account);
      const applied = withFormula(applyOperations(source, fieldOperations(draft, model, type)), draft.name, draft.expression);
      if (applied.refused.length > 0) {
        setRefusals(applied.refused.map((r) => r.reason));
        return;
      }
      const answer = await writeModelSource({ project, name: modelName, source: applied.source, dryRun: false });
      if (answer.kind !== "proposed") {
        if (answer.kind === "refused") {
          setRefusals([answer.problem.detail ?? answer.problem.title ?? t("spaces.fields.refusedStatus", { status: answer.status })]);
        }
        return;
      }
      onProposed(answer.change);
      const manifests = formulaManifests(
        { project, space, type, orgDomain, endpoint: through.metadata.name },
        checkFormulas(formulaSlots(draft, model, type)),
      );
      const body = { manifests, conflictPolicy: "replace" as const };
      try {
        const pipeline = await importChecked(project, body);
        if (isChange(pipeline)) onProposed(pipeline);
        close(false);
      } catch (error) {
        setRefusals([t("spaces.fields.formula.pipelineRefused", { reason: error instanceof Error ? error.message : String(error) })]);
      }
    } catch (error) {
      setRefusals([error instanceof Error ? error.message : String(error)]);
    } finally {
      setBusy(false);
    }
  }

  function submit() {
    setTried(true);
    if (Object.keys(problems).length > 0) return;
    if (draft.type === "formula") void proposeFormula();
    else void propose(applyOperations(source, fieldOperations(draft, model, type)));
  }

  const message = (key: string | undefined) => (key ? [t(`spaces.fields.problem.${key}`)] : undefined);
  const classes = model.classes.map((c) => c.name);

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={t("spaces.fields.title", { type })}
      description={t("spaces.fields.lead")}
      closeLabel={t("app.close")}
      footer={
        <>
          <Button variant="secondary" onClick={() => close(false)}>
            {t("app.cancel")}
          </Button>
          <Button variant="primary" onClick={submit} disabled={busy} disabledReason={t("app.busyReason")}>
            {t("spaces.fields.propose")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        {refusals.length > 0 ? (
          <Alert tone="danger" title={t("spaces.fields.refused")}>
            <ul>
              {refusals.map((reason) => (
                <li key={reason}>{reason}</li>
              ))}
            </ul>
          </Alert>
        ) : null}

        <section aria-labelledby={`${id}-sdm`} className="flex flex-col gap-2">
          <h3 id={`${id}-sdm`} className="text-body font-semibold">
            {t("spaces.fields.sdmTitle")}
          </h3>
          {official.isPending ? <p role="status">{t("app.loading")}</p> : null}
          {official.isError ? <p className="text-body text-fg-muted">{t("spaces.fields.sdmUnavailable")}</p> : null}
          {official.isSuccess && suggestions.length === 0 ? (
            <p className="text-body text-fg-muted">{t(official.data ? "spaces.fields.sdmNone" : "spaces.fields.sdmNoModel", { type })}</p>
          ) : null}
          {suggestions.length > 0 ? (
            <ul className="flex flex-col gap-2">
              {suggestions.map((slot: LinkmlSlot) => (
                <li key={slot.name} className="flex flex-wrap items-center gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={busy} disabledReason={t("app.busyReason")}
                    aria-label={t("spaces.fields.proposeOfficial", { name: slot.name })}
                    onClick={() => official.data && void propose(withOfficialSlot(source, slot, official.data, model, type))}
                  >
                    {slot.name}
                  </Button>
                  {slot.description ? <span className="text-caption text-fg-muted">{slot.description}</span> : null}
                </li>
              ))}
            </ul>
          ) : null}
        </section>

        <section aria-labelledby={`${id}-own`} className="flex flex-col gap-3">
          <h3 id={`${id}-own`} className="text-body font-semibold">
            {t("spaces.fields.ownTitle")}
          </h3>
          <Field id={`${id}-name`} label={t("spaces.fields.name")} required help={t("spaces.fields.nameHelp")} errors={message(shown.name)}>
            <Input id={`${id}-name`} value={draft.name} autoComplete="off" onChange={(e) => set({ name: e.target.value.trim() })} />
          </Field>
          <Field id={`${id}-type`} label={t("spaces.fields.type")}>
            <Select id={`${id}-type`} value={draft.type} onChange={(e) => set({ type: e.target.value as FieldType })}>
              {FIELD_TYPES.map((each) => (
                <option key={each} value={each}>
                  {t(`spaces.fields.types.${each}`)}
                </option>
              ))}
            </Select>
          </Field>
          {draft.type === "select" || draft.type === "multiSelect" ? (
            <Field id={`${id}-values`} label={t("spaces.fields.values")} required help={t("spaces.fields.valuesHelp")} errors={message(shown.values)}>
              <Textarea
                id={`${id}-values`}
                rows={4}
                value={valuesText}
                onChange={(e) => {
                  setValuesText(e.target.value);
                  set({ values: e.target.value.split("\n").map((v) => v.trim()).filter((v) => v !== "") });
                }}
              />
            </Field>
          ) : null}
          {draft.type === "relationship" ? (
            <>
              <Field id={`${id}-target`} label={t("spaces.fields.target")} required errors={message(shown.target)}>
                <Select id={`${id}-target`} value={draft.target} onChange={(e) => set({ target: e.target.value })}>
                  <option value="">—</option>
                  {classes.map((each) => (
                    <option key={each} value={each}>
                      {each}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field id={`${id}-inverse`} label={t("spaces.fields.inverse")} required help={t("spaces.fields.inverseHelp", { type })} errors={message(shown.inverse)}>
                <Input id={`${id}-inverse`} value={draft.inverse} autoComplete="off" onChange={(e) => set({ inverse: e.target.value.trim() })} />
              </Field>
              <Checkbox label={t("spaces.fields.many")} checked={draft.many} onChange={(e) => set({ many: e.target.checked })} />
            </>
          ) : null}
          {draft.type === "formula" ? (
            <Field
              id={`${id}-expression`}
              label={t("spaces.fields.formula.expression")}
              required
              help={t("spaces.fields.formula.help", {
                example: "{capacity} - {available}",
                slots: formulaSlots(draft, model, type)
                  .filter((s) => s.name !== draft.name && s.kind === "Property")
                  .map((s) => `{${s.name}}`)
                  .join(", "),
              })}
              errors={shown.expression ? [t("spaces.fields.problem.formulaInvalid", { reason: formulaProblem(draft, model, type) ?? "" })] : undefined}
            >
              <Textarea id={`${id}-expression`} rows={3} value={draft.expression} spellCheck={false} className="font-mono" onChange={(e) => set({ expression: e.target.value })} />
            </Field>
          ) : (
            <Checkbox label={t("spaces.fields.required")} checked={draft.required} onChange={(e) => set({ required: e.target.checked })} />
          )}
        </section>
      </div>
    </Dialog>
  );
}
