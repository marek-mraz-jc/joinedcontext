/**
 * Upload a CSV into a Context Space (DM-81, T-3840): the file is ranked against the Smart Data
 * Models types of the city subjects, the person takes a type or their own, `jc_space_complete`
 * proposes the model, the space, its Endpoint and the person's write grant as one Change, and once
 * that Change is live {@link LoadRows} upserts the rows through the gateway with the person's own session.
 */
import { useEffect, useEffectEvent, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { callOperation } from "../../api/operations";
import type { Change } from "../../api/manifest";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { usePermissions } from "../../api/permissions";
import { Alert, Button, Card, Field, FileDropZone, Input, PageHeader, RadioGroup } from "../../components/ui";
import { CITY_SUBJECTS, SAMPLE_BYTES, modelsOf, rankModels, sampleOf, spaceNameOf, tableNameOf, typeNameOf, uploadInput } from "./csvUpload";
import { DraftedModel } from "./DraftedModel";
import type { CatalogueModel, SdmMatch } from "./csvUpload";
import { csvOf, decodeAs, MAX_ROWS, readSheets, sheetFileName, unreadableText } from "./importRows";
import type { Sheet, Table } from "./importRows";
import { LoadRows } from "./LoadRows";
import { SheetChoice } from "./SheetChoice";

/** A type name the person writes for their own model: PascalCase, as the op names the class. */
const TYPE_NAME = /^[A-Z][A-Za-z0-9]{0,62}$/;

/**
 * A file read: its sheets (one for a CSV), the one the upload takes, the sample `jc_space_complete`
 * infers the model from and the name it travels under, and the types ranked against its columns.
 */
type Read = { sheets: Sheet[]; sheet: string; table: Table; name: string; fileName: string; sample: string; models: CatalogueModel[]; matches: SdmMatch[] };

/** One sheet of a workbook as a read: an Excel sheet reaches the op as the CSV of its rows (JC1-07). */
function sheetRead(read: Omit<Read, "sheet" | "table" | "fileName" | "sample" | "matches">, sheet: Sheet, sample: string | undefined): Read {
  return {
    ...read,
    sheet: sheet.name,
    table: sheet.table,
    fileName: sample === undefined ? sheetFileName(read.name, read.sheets, sheet) : read.name,
    sample: sample ?? sampleOf(csvOf(sheet.table)),
    matches: rankModels(sheet.table.columns, read.models),
  };
}
type Proposed = { space: string; type: string; source: string | undefined; change: Change | null };
type Completed = { space: string; drafts: { kind: string; manifest: { spec?: { source?: unknown } } }[]; change?: Change | null };

/** The LinkML of the model the op drafted, when it drafted one. */
function draftedSource(out: Completed): string | undefined {
  const drafted = out.drafts.find((d) => d.kind === "DataModel")?.manifest.spec?.source;
  return typeof drafted === "string" ? drafted : undefined;
}

/** A Smart Data Models type as LinkML, the model a chosen match proposes. */
async function sdmLinkml(model: string): Promise<string | undefined> {
  return ((await unwrap(await api.POST("/api/v1/tools/import-sdm", { body: { model } }))) as { linkml?: string }).linkml;
}

/** What the page does with the file, before one is chosen (T-6567). */
function Steps(): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2">
      <p className="text-body font-semibold text-fg">{t("spaces.upload.steps.title")}</p>
      <ol className="flex list-decimal flex-col gap-1 pl-5 text-body text-fg-muted">
        <li>{t("spaces.upload.steps.model")}</li>
        <li>{t("spaces.upload.steps.space")}</li>
        <li>{t("spaces.upload.steps.rows")}</li>
      </ol>
    </div>
  );
}

export function UploadCsv({ project, space: given = "" }: { project: string; space?: string }): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(project);
  const [read, setRead] = useState<Read | null>(null);
  // The chosen Smart Data Models id, or "" for the person's own model.
  const [choice, setChoice] = useState("");
  const [ownType, setOwnType] = useState("");
  const [space, setSpace] = useState(given);
  const [busy, setBusy] = useState(false);
  // The file's name while it is read and ranked: the catalogue can take seconds (T-5109).
  const [reading, setReading] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [proposed, setProposed] = useState<Proposed | null>(null);

  const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

  const choose = async (file: File) => {
    setProblem(null);
    setRead(null);
    setProposed(null);
    setReading(file.name);
    try {
      const sheets = (await readSheets(file)).filter((sheet) => sheet.table.rows.length > 0);
      const first = sheets[0];
      if (!first) {
        setProblem(t("spaces.import.empty"));
        return;
      }
      const workbook = file.name.toLowerCase().endsWith(".xlsx");
      // A text file's sample is read in the encoding the table was, or a Windows-1250 header
      // reaches the model inference garbled and its slots no longer match the columns (F-UI3-013).
      const sample = workbook ? undefined : sampleOf(decodeAs(new Uint8Array(await file.slice(0, SAMPLE_BYTES).arrayBuffer()), first.table.detected?.encoding));
      // The catalogue is a help, not a gate: unreachable, the file still gets its own model.
      let models: CatalogueModel[] = [];
      try {
        const catalogue = await unwrap(
          await api.GET("/api/v1/tools/sdm-catalog", { params: { query: { subject: CITY_SUBJECTS.join(",") } } }),
        );
        models = modelsOf(catalogue as Parameters<typeof modelsOf>[0]);
      } catch {
        models = [];
      }
      const taken = sheetRead({ sheets, name: file.name, models }, first, sample);
      setRead(taken);
      setChoice(taken.matches[0]?.model.id ?? "");
      // Both names are suggested from the table, so one click proposes; the person may change them.
      const title = tableNameOf(file.name, first.name);
      setOwnType(typeNameOf(title) ?? "");
      setSpace((now) => (now.trim() === "" ? spaceNameOf(title) : now));
    } catch (error) {
      setProblem(unreadableText(t, error));
    } finally {
      setReading(null);
    }
  };

  // Another sheet is another table: its columns are ranked again and its type chosen afresh.
  const takeSheet = (name: string) => {
    const sheet = read?.sheets.find((s) => s.name === name);
    if (!read || !sheet) return;
    const taken = sheetRead(read, sheet, undefined);
    setRead(taken);
    setChoice(taken.matches[0]?.model.id ?? "");
    if (ownType === (typeNameOf(tableNameOf(read.name, read.sheet)) ?? "")) setOwnType(typeNameOf(tableNameOf(read.name, name)) ?? "");
  };

  const match = read?.matches.find((m) => m.model.id === choice);
  const typeName = match ? match.model.name : ownType.trim();
  const ready = read !== null && space.trim() !== "" && TYPE_NAME.test(typeName);
  const missing = read === null || ready ? undefined : !TYPE_NAME.test(typeName) ? t("spaces.upload.missingType") : t("spaces.upload.missingSpace");

  // The model is drafted as the person reads the form, without proposing, so its picture stands
  // beside the columns before Propose (T-6567). A draft for another input is not shown.
  const [draft, setDraft] = useState<{ key: string; source: string } | null>(null);
  const draftKey = read && ready && !proposed ? [read.fileName, read.sheet, match?.model.id ?? "", typeName, space.trim()].join("\n") : "";
  const drawDraft = useEffectEvent(async (key: string) => {
    if (!read) return;
    const linkml = match ? await sdmLinkml(match.model.id) : undefined;
    const answer = await callOperation(project, "jc_space_complete", {
      ...uploadInput({ space: space.trim(), fileName: read.fileName, sample: read.sample, typeName, linkml }),
      propose: false,
    });
    const source = linkml ?? (answer.ok ? draftedSource(answer.output as Completed) : undefined);
    if (source !== undefined) setDraft({ key, source });
  });
  useEffect(() => {
    if (draftKey === "") return;
    // A pause in typing, not every key, asks for a draft.
    const timer = window.setTimeout(() => void drawDraft(draftKey).catch(() => undefined), 400);
    return () => window.clearTimeout(timer);
  }, [draftKey]);

  const propose = async () => {
    if (!read || !ready) return;
    setBusy(true);
    setProblem(null);
    try {
      const linkml = match ? await sdmLinkml(match.model.id) : undefined;
      const answer = await callOperation(
        project,
        "jc_space_complete",
        uploadInput({ space: space.trim(), fileName: read.fileName, sample: read.sample, typeName, linkml }),
      );
      if (!answer.ok) {
        setProblem(answer.reason ?? `HTTP ${answer.status}`);
        return;
      }
      const out = answer.output as Completed;
      setProposed({ space: out.space, type: typeName, source: linkml ?? draftedSource(out), change: out.change ?? null });
    } catch (error) {
      setProblem(reason(error));
    } finally {
      setBusy(false);
    }
  };

  // A role that may not propose a space reads why, not a form whose every step ends in a 403 (T-4263).
  if (permissions.data !== undefined && !permissions.can("ContextSpace", "propose")) {
    return (
      <div className="flex flex-col gap-section">
        <PageHeader title={t("spaces.upload.title")} description={t("spaces.upload.lead")} />
        <Alert role="status" tone="info">
          {t("form.mayNotCreate")}
        </Alert>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-section">
      <PageHeader title={t("spaces.upload.title")} description={t("spaces.upload.lead")} />
      {problem ? (
        <Alert role="alert" tone="danger">
          {problem}
        </Alert>
      ) : null}
      <Card className="flex flex-col gap-4">
        <FileDropZone
          label={t("spaces.upload.choose")}
          button={t("spaces.upload.chooseButton")}
          hint={t("spaces.upload.chooseHint")}
          accept=".csv,.tsv,.txt,.xlsx"
          primary={read === null}
          intro={read === null && reading === null ? <Steps /> : undefined}
          onFiles={(files) => void choose(files[0])}
        />
        {reading ? (
          <p role="status" className="text-body text-fg-muted">
            {t("spaces.upload.reading", { name: reading })}
          </p>
        ) : null}
        {read ? (
          <p className="text-body text-fg-muted">
            {t("spaces.import.read", { name: read.sheets.length > 1 ? `${read.name} (${read.sheet})` : read.name, rows: read.table.rows.length, columns: read.table.columns.length })}
          </p>
        ) : null}
        {read ? <p className="text-caption text-fg-muted">{t("spaces.upload.columns", { names: read.table.columns.join(", ") })}</p> : null}
        {read && !proposed ? <SheetChoice id="upload-sheet" sheets={read.sheets} value={read.sheet} onChange={takeSheet} /> : null}
        {read?.table.total !== undefined ? <Alert tone="warning">{t("spaces.import.cut", { total: read.table.total, max: MAX_ROWS })}</Alert> : null}
      </Card>
      {read && !proposed ? (
        // The form and the model it makes side by side, so a changed type name is seen redrawn.
        <div className="grid items-start gap-section lg:grid-cols-2">
        <Card className="flex flex-col gap-4">
          {read.matches.length === 0 ? <p className="text-body text-fg-muted">{t("spaces.upload.noMatch")}</p> : null}
          {/* With no Smart Data Models type to choose, a one-option radio is no choice: the type name alone. */}
          {read.matches.length > 0 ? (
          <RadioGroup
            name="upload-type"
            legend={t("spaces.upload.type")}
            value={choice}
            onChange={setChoice}
            options={[
              ...read.matches.map((m) => ({
                value: m.model.id,
                label: t("spaces.upload.match", { name: m.model.name, score: Math.round(m.score * 100) }),
                description: Object.entries(m.mapping)
                  .map(([column, attribute]) => `${column} → ${attribute}`)
                  .join(", "),
              })),
              { value: "", label: t("spaces.upload.own") },
            ]}
          />
          ) : null}
          {choice === "" ? (
            <Field id="upload-own-type" label={t("spaces.upload.ownType")} help={t("spaces.upload.ownTypeHint")}>
              <Input id="upload-own-type" className="font-mono" value={ownType} onChange={(e) => setOwnType(e.target.value)} />
            </Field>
          ) : null}
          <Field id="upload-space" label={t("spaces.upload.space")} help={t("spaces.upload.spaceHint")}>
            <ResourceNamePicker id="upload-space" label={t("spaces.upload.space")} labelled from={{ project, plural: "spaces" }} value={space} onChange={setSpace} create />
          </Field>
          <Button className="w-fit" variant="primary" onClick={() => void propose()} disabled={!ready || busy} disabledReason={missing ?? t("spaces.upload.notReady")}>
            {busy ? t("spaces.upload.proposing") : t("spaces.upload.propose")}
          </Button>
          {/* What still keeps Propose out of reach, on screen and not only for a screen reader. */}
          {missing ? (
            <p className="text-caption text-fg-muted" aria-hidden="true">
              {missing}
            </p>
          ) : null}
        </Card>
        {draft?.key === draftKey ? (
          // On a phone the model comes first: it is what Propose sends.
          <Card className="order-first lg:order-none">
            <DraftedModel source={draft.source} type={typeName} table={read.table} />
          </Card>
        ) : null}
        </div>
      ) : null}
      {proposed && read ? <LoadRows project={project} space={proposed.space} type={proposed.type} source={proposed.source} table={read.table} change={proposed.change} /> : null}
      {proposed && read ? (
        <Card>
          <DraftedModel source={proposed.source} type={proposed.type} table={read.table} />
        </Card>
      ) : null}
    </div>
  );
}
