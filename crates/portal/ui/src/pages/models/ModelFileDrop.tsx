import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import { callOperation } from "../../api/operations";
import { nameFrom } from "../datasources/templates";
import { useOrgDomain } from "../../api/projects";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  Dialog,
  Field,
  FilePicker,
  Icon,
  Input,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { blankSource, edit, UPSTREAM_ANNOTATION } from "./linkml";
import { applyOperations } from "./operations";
import type { Operation, Refusal } from "./operations";

/**
 * A model from a dropped file (T-0599, DM-54, DM-55): the sample goes to the Portal, Model
 * Tools reads it once in memory and answers a draft as editor operations; the preview shows
 * what was inferred, and "Populate the editor" applies the operations to a blank model through
 * the same `applyOperations` a person's clicks go through (DM-13). Every string of the answer
 * is text from the file and is rendered as text, never as markup.
 */

export const MAX_SAMPLE_BYTES = 10 * 1024 * 1024;
export const ACCEPT = ".csv,.tsv,.xlsx,.json,.pdf,text/csv,application/json,application/pdf";

export interface InferAnswer {
  linkml: string;
  operations: Operation[];
  detectedTypes: Record<string, string>;
  matches: Record<string, { model: string; slotUri: string }>;
  untyped: { slot: string; reason: string }[];
  rows: number;
}

/** The answer of `POST /api/v1/tools/infer-schema`, or none when it is not what Model Tools writes. */
export function inferAnswerOf(value: unknown): InferAnswer | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const answer = value as Record<string, unknown>;
  if (typeof answer.linkml !== "string" || !Array.isArray(answer.operations)) {
    return null;
  }
  const operations = answer.operations.filter(
    (op): op is Operation => typeof op === "object" && op !== null && typeof (op as { op?: unknown }).op === "string",
  );
  const matches: InferAnswer["matches"] = {};
  for (const [slot, match] of Object.entries(record(answer.matches))) {
    const entry = record(match);
    if (typeof entry.model === "string" && typeof entry.slotUri === "string") {
      matches[slot] = { model: entry.model, slotUri: entry.slotUri };
    }
  }
  const untyped = (Array.isArray(answer.untyped) ? answer.untyped : [])
    .map(record)
    .filter((entry) => typeof entry.slot === "string")
    .map((entry) => ({ slot: String(entry.slot), reason: typeof entry.reason === "string" ? entry.reason : "" }));
  const detectedTypes: Record<string, string> = {};
  for (const [slot, type] of Object.entries(record(answer.detectedTypes))) {
    if (typeof type === "string") {
      detectedTypes[slot] = type;
    }
  }
  return {
    linkml: answer.linkml,
    operations,
    detectedTypes,
    matches,
    untyped,
    rows: typeof answer.rows === "number" ? answer.rows : 0,
  };
}

/** The upload `tools/infer-schema` reads: the sample under `file`, with its own name. */
export function inferForm(file: File): FormData {
  const form = new FormData();
  form.append("file", file, file.name);
  return form;
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

export interface Draft {
  name: string;
  answer: InferAnswer;
  /** The blank model with the operations applied: what the editor opens with. */
  source: string;
  refused: Refusal[];
}

/** The draft as the editor will hold it: the operations applied to a blank model of the organisation. */
export function draftOf(answer: InferAnswer, domain: string, fileName: string): Draft {
  const stem =
    fileName
      .replace(/\.[^.]+$/, "")
      .replace(/[^A-Za-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .toLowerCase() || "sample";
  const applied = applyOperations(blankSource(domain, stem), answer.operations);
  return { name: fileName, answer, source: cited(applied.source, answer.matches), refused: applied.refused };
}

/** The namespace a Smart Data Models match's IRI lies under, declared as `sdm` beside it. */
const SDM = "https://smartdatamodels.org/";

/**
 * The draft with each Smart Data Models match cited as the inferred model cites it (JC1-20):
 * the slot's upstream IRI and the model it is borrowed from. The operations leave both out,
 * since the editor refuses a reserved IRI by operation, so they are written here; a match
 * outside the Smart Data Models namespace or for a slot the draft lacks is left out.
 */
export function cited(source: string, matches: InferAnswer["matches"]): string {
  const taken = Object.entries(matches).filter(([, match]) => match.slotUri.startsWith(SDM));
  if (taken.length === 0) return source;
  return edit(source, (document) => {
    let any = false;
    for (const [slot, match] of taken) {
      if (!document.hasIn(["slots", slot])) continue;
      document.setIn(["slots", slot, "slot_uri"], match.slotUri);
      document.setIn(["slots", slot, "annotations", UPSTREAM_ANNOTATION], match.model);
      any = true;
    }
    if (any && !document.hasIn(["prefixes", "sdm"])) document.setIn(["prefixes", "sdm"], SDM);
  });
}

interface SlotRow {
  klass: string;
  name: string;
  title?: string;
  range?: string;
  kind?: string;
  unit?: string;
}

/** The slots of the draft, one row each, as the operations declare them. */
export function slotRows(operations: Operation[]): SlotRow[] {
  const rows: SlotRow[] = [];
  let klass = "";
  for (const op of operations) {
    if (op.op === "addClass") {
      klass = op.name;
    } else if (op.op === "addSlot") {
      rows.push({ klass: op.class ?? klass, name: op.name, range: op.range, kind: op.kind });
    } else if (op.op === "setTitle" && op.target === "slot") {
      const row = rows.find((r) => r.name === op.name);
      if (row) {
        row.title = op.value;
      }
    } else if (op.op === "setSlot" && op.field === "unit") {
      const row = rows.find((r) => r.name === op.name);
      if (row) {
        row.unit = String(op.value);
      }
    }
  }
  return rows;
}

/**
 * Whether an address carries a credential: a user name or password, or a query parameter named
 * like one. A courtesy before anything is sent; `credential_in_address` in `src/ops/compute.rs`
 * is the refusal that counts (T-3250).
 */
export function credentialInAddress(url: URL): boolean {
  const names = ["password", "token", "secret", "clientsecret", "apikey", "apitoken", "accesstoken", "key", "sig", "signature", "auth", "credential"];
  return (
    url.username !== "" ||
    url.password !== "" ||
    [...url.searchParams.keys()].some((name) => names.includes(name.replace(/[-_]/g, "").toLowerCase()))
  );
}

export function ModelFileDrop({
  project,
  onPopulate,
  onTable,
  compact = false,
  icon = false,
}: {
  project: string;
  /** The draft source the editor opens with, and the answer it came from. */
  onPopulate: (source: string, answer: InferAnswer) => void;
  /** Takes a CSV or Excel table instead of drafting a model from it (T-6445). */
  onTable?: (file: File) => void;
  /** The label and input only, for a dock with no room for the lead. */
  compact?: boolean;
  /** A paperclip button only, for a chat composer. */
  icon?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  // The inferred model is minted under the organization's domain, as a hand-written one is.
  const orgDomain = useOrgDomain(project);
  const [busy, setBusy] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);

  const takeFile = async (file: File | undefined): Promise<void> => {
    if (!file) {
      return;
    }
    setProblem(null);
    if (file.size > MAX_SAMPLE_BYTES) {
      setProblem(t("models.infer.tooLarge"));
      return;
    }
    if (onTable && /\.(csv|tsv|xlsx)$/i.test(file.name)) {
      onTable(file);
      return;
    }
    const form = inferForm(file);
    setBusy(file.name);
    try {
      // Through the typed client (UI-07). The document types this multipart body as an object;
      // the route reads a form with one `file` part, and the client passes a FormData through
      // untouched so the browser writes the boundary.
      const { data, error, response } = await api.POST("/api/v1/tools/infer-schema", {
        body: form as unknown as Record<string, undefined>,
      });
      if (data === undefined) {
        const detail = record(error).detail;
        setProblem(t("models.infer.failed", { detail: typeof detail === "string" ? detail : String(response.status) }));
        return;
      }
      const answer = inferAnswerOf(data);
      if (!answer) {
        setProblem(t("models.infer.unreadable"));
        return;
      }
      setDraft(draftOf(answer, orgDomain, file.name));
    } catch {
      setProblem(t("models.infer.failed", { detail: "network" }));
    } finally {
      setBusy(null);
    }
  };

  // An API's address instead of a file (T-3250): fetched once on the project's runner by
  // `jc_model_infer`, as a data source's Check fetches it, and inferred from its records.
  const [address, setAddress] = useState("");
  const takeAddress = async (): Promise<void> => {
    setProblem(null);
    let url: URL;
    try {
      url = new URL(address.trim());
    } catch {
      setProblem(t("models.infer.urlInvalid"));
      return;
    }
    if (url.protocol !== "https:" && url.protocol !== "http:") {
      setProblem(t("models.infer.urlInvalid"));
      return;
    }
    if (credentialInAddress(url)) {
      setProblem(t("models.infer.urlCredential"));
      return;
    }
    const name = nameFrom(url.toString()) || "sample";
    setBusy(url.host);
    try {
      const answered = await callOperation(project, "jc_model_infer", { url: url.toString(), name });
      if (!answered.ok) {
        setProblem(t("models.infer.failed", { detail: answered.reason ?? String(answered.status) }));
        return;
      }
      const answer = inferAnswerOf(answered.output);
      if (!answer) {
        setProblem(t("models.infer.unreadable"));
        return;
      }
      setDraft(draftOf(answer, orgDomain, name));
    } catch {
      setProblem(t("models.infer.failed", { detail: "network" }));
    } finally {
      setBusy(null);
    }
  };

  const rows = draft ? slotRows(draft.answer.operations) : [];
  const classes = draft ? draft.answer.operations.filter((op) => op.op === "addClass").length : 0;


  return (
    <section
      aria-label={t("models.infer.title")}
      className={
        icon
          ? "relative flex"
          : compact
            ? "flex flex-col gap-1"
            : "flex flex-col gap-2 rounded-md border border-dashed border-border bg-surface-subtle p-3"
      }
      onDragOver={(event) => event.preventDefault()}
      onDrop={(event) => {
        event.preventDefault();
        void takeFile(event.dataTransfer.files[0]);
      }}
    >
      {compact || icon ? null : (
        <>
          <h2 className="text-body font-semibold text-fg">{t("models.infer.title")}</h2>
          <p className="text-caption text-fg-muted">{t("models.infer.lead")}</p>
        </>
      )}
      {icon ? (
        <FilePicker
          label={t("assistant.attach")}
          accept={ACCEPT}
          onFile={(file) => void takeFile(file)}
          title={busy ? t("models.infer.reading", { name: busy }) : t("assistant.attach")}
          className="rounded p-2 text-fg-muted hover:bg-surface-subtle hover:text-fg"
        >
          <Icon name="paperclip" className={busy ? "size-5 motion-safe:animate-pulse" : "size-5"} />
        </FilePicker>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <FilePicker
            label={t("models.infer.chooseFile")}
            accept={ACCEPT}
            onFile={(file) => void takeFile(file)}
          >
            <span className={buttonClass("secondary", "md", "cursor-pointer")}>{t("models.infer.drop")}</span>
          </FilePicker>
          {busy ? (
            <span role="status" className="text-caption text-fg-muted">
              {t("models.infer.reading", { name: busy })}
            </span>
          ) : null}
        </div>
      )}
      {compact || icon ? null : (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void takeAddress();
          }}
        >
          <Field id="models-infer-url" label={t("models.infer.url")} help={t("models.infer.urlHelp")} className="min-w-64 flex-1">
            <Input
              id="models-infer-url"
              type="url"
              inputMode="url"
              value={address}
              onChange={(event) => setAddress(event.target.value)}
            />
          </Field>
          <Button
            type="submit"
            disabled={address.trim() === "" || busy !== null}
            disabledReason={busy !== null ? t("app.busyReason") : t("models.infer.enterAddress")}
          >
            {t("models.infer.readUrl")}
          </Button>
        </form>
      )}
      {problem ? (
        icon ? (
          <Alert tone="danger" role="alert" className="absolute bottom-full left-0 z-10 mb-1 w-64 shadow-2">
            {problem}
          </Alert>
        ) : (
          <Alert tone="danger" role="alert">
            {problem}
          </Alert>
        )
      ) : null}
      <Dialog
        open={draft !== null}
        onOpenChange={(open) => {
          if (!open) {
            setDraft(null);
          }
        }}
        title={t("models.infer.preview", { name: draft?.name ?? "" })}
        description={t("models.infer.previewLead", { rows: draft?.answer.rows ?? 0, classes })}
        size="lg"
        closeLabel={t("models.infer.close")}
        footer={
          <>
            <Button
              onClick={() => {
                setDraft(null);
              }}
            >
              {t("models.infer.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={draft === null || draft.refused.length > 0}
              disabledReason={
                draft && draft.refused.length > 0
                  ? t("models.infer.refused", { n: draft.refused.length })
                  : undefined
              }
              onClick={() => {
                if (draft) {
                  onPopulate(draft.source, draft.answer);
                  setDraft(null);
                }
              }}
            >
              {t("models.infer.populate")}
            </Button>
          </>
        }
      >
        {draft ? (
          <div className="flex flex-col gap-3 text-body">
            <Table caption={t("models.infer.title")}>
              <TableHead>
                <TableHeaderCell>{t("models.infer.class")}</TableHeaderCell>
                <TableHeaderCell>{t("models.infer.column")}</TableHeaderCell>
                <TableHeaderCell>{t("models.infer.slot")}</TableHeaderCell>
                <TableHeaderCell>{t("models.infer.range")}</TableHeaderCell>
                <TableHeaderCell>{t("models.infer.kind")}</TableHeaderCell>
                <TableHeaderCell>{t("models.infer.unit")}</TableHeaderCell>
                <TableHeaderCell>{t("models.infer.match")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {rows.map((row) => (
                  <TableRow key={`${row.klass}/${row.name}`}>
                    <TableCell className="font-mono">{row.klass}</TableCell>
                    <TableCell className="max-w-64 break-words">{row.title ?? row.name}</TableCell>
                    <TableCell className="font-mono">{row.name}</TableCell>
                    <TableCell>
                      <Badge mono>{row.range ?? "string"}</Badge>
                    </TableCell>
                    <TableCell>{row.kind ?? "Property"}</TableCell>
                    <TableCell className="font-mono">{row.unit ?? ""}</TableCell>
                    <TableCell className="break-all font-mono">{draft.answer.matches[row.name]?.model ?? ""}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            {draft.answer.untyped.length > 0 ? (
              <div>
                <p className="font-medium">{t("models.infer.untyped")}</p>
                <ul className="list-disc pl-5 text-caption">
                  {draft.answer.untyped.map((entry) => (
                    <li key={entry.slot}>
                      <span className="font-mono">{entry.slot}</span>: {entry.reason}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
            {draft.refused.length > 0 ? (
              <Alert tone="danger" role="alert">
                <p>{t("models.infer.refused", { n: draft.refused.length })}</p>
                <ul className="list-disc pl-5">
                  {draft.refused.map((refusal) => (
                    <li key={refusal.index}>{refusal.reason}</li>
                  ))}
                </ul>
              </Alert>
            ) : null}
          </div>
        ) : null}
      </Dialog>
    </section>
  );
}
