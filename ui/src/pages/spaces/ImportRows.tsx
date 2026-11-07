/**
 * Import and export on a space's data (T-3109, ADR-N-042 §3.6). The import is a wizard: a file,
 * its columns mapped to the type's slots, the rows checked, then created in batches through the
 * gateway with the person's session; what was refused is listed and can be downloaded. The export
 * is a link to an Endpoint's file of the type, narrowed to the view's filter.
 */
import { useId, useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { download } from "@joinedcontext/sdk";
import { Alert, Button, Dialog, Field, FilePicker, Input, RadioGroup, Select, safeHref } from "../../components/ui";
import { RetentionGuide } from "../../components/RetentionGuide";
import { Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../../components/ui/Table";
import type { Manifest } from "../../api/manifest";
import { servedRepresentations } from "../../components/endpoints/links";
import { effectiveHistory, fromIso, HISTORY_FOREVER, HISTORY_OFF } from "./retention";
import type { RetentionRule } from "./retention";
import {
  EXPIRES_AT,
  EXPORTS,
  ID,
  MAX_ROWS,
  createInBatches,
  exportUrl,
  readTable,
  rejectedCsv,
  suggestMapping,
  toEntities,
  unreadableText,
} from "./importRows";
import type { ImportSlot, Mapping, Rejected, Send, Table as Rows, Target } from "./importRows";

/** How many rows the mapping step shows of the file. */
const PREVIEW = 5;
/** How many rows the preview grid shows (T-3248). */
const PREVIEW_ROWS = 50;

/** How many refused rows the report lists on screen; the download holds them all. */
const LISTED = 50;

type Step =
  | { kind: "file" }
  | { kind: "map"; table: Rows; mapping: Mapping; name: string; expiresAt?: string }
  | { kind: "writing"; sent: number; total: number }
  | { kind: "done"; created: number; rejected: Rejected[]; denied?: string };

export function ImportRowsDialog({
  open,
  onOpenChange,
  target,
  slots,
  send,
  onImported,
  retention = [],
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  target: Target;
  slots: ImportSlot[];
  /** The person's own transport to the gateway, the one every edit of the page uses. */
  send: Send;
  /** Called once rows were created, so the page reads the type again. */
  onImported: () => void;
  /** The space's `spec.retention`, to say what its history rule does to the rows (T-4822). */
  retention?: RetentionRule[];
}): JSX.Element {
  const { t } = useTranslation();
  const [step, setStep] = useState<Step>({ kind: "file" });
  const [problem, setProblem] = useState<string | null>(null);

  const checked = useMemo(
    () => (step.kind === "map" ? toEntities(step.table, step.mapping, slots, target, undefined, step.expiresAt) : null),
    [step, slots, target],
  );

  const close = (next: boolean) => {
    onOpenChange(next);
    if (!next) {
      setStep({ kind: "file" });
      setProblem(null);
    }
  };

  const choose = async (file: File) => {
    setProblem(null);
    try {
      const table = await readTable(file);
      if (table.rows.length === 0) {
        setProblem(t("spaces.import.empty"));
        return;
      }
      setStep({ kind: "map", table, mapping: suggestMapping(table.columns, slots), name: file.name });
    } catch (error) {
      setProblem(unreadableText(t, error));
    }
  };

  const write = async () => {
    if (step.kind !== "map" || !checked) return;
    const total = checked.entities.length;
    setStep({ kind: "writing", sent: 0, total });
    const answer = await createInBatches(send, target.space, checked.entities, (sent) =>
      setStep({ kind: "writing", sent, total }),
    );
    const rejected = [...checked.rejected, ...answer.rejected].sort((a, b) => a.row - b.row);
    setStep({ kind: "done", created: answer.created, rejected, denied: answer.denied });
    if (answer.created > 0) onImported();
  };

  const footer =
    step.kind === "map" && checked ? (
      <>
        <Button variant="secondary" onClick={() => setStep({ kind: "file" })}>
          {t("spaces.import.back")}
        </Button>
        <Button
          onClick={() => void write()}
          disabled={checked.entities.length === 0}
          disabledReason={t("spaces.import.nothingToCreate")}
        >
          {t("spaces.import.create", { count: checked.entities.length })}
        </Button>
      </>
    ) : step.kind === "done" ? (
      <Button onClick={() => close(false)}>{t("spaces.import.close")}</Button>
    ) : undefined;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="lg"
      title={t("spaces.import.title", { type: target.type })}
      description={t("spaces.import.lead")}
      closeLabel={t("spaces.import.close")}
      footer={footer}
    >
      {problem ? (
        <Alert role="alert" tone="danger">
          {problem}
        </Alert>
      ) : null}
      {step.kind === "file" ? (
        <FilePicker
          label={t("spaces.import.choose")}
          accept=".csv,.tsv,.txt,.xlsx,.json,.geojson,.xml"
          onFile={(file) => void choose(file)}
        >
          <span className="inline-flex items-center rounded-md border border-dashed border-border px-4 py-3 text-body">
            {t("spaces.import.choose")}
          </span>
        </FilePicker>
      ) : null}
      {step.kind === "map" && checked ? (
        <MapStep
          step={step}
          slots={slots}
          valid={checked.entities.length}
          refused={checked.rejected}
          onMapping={(mapping) => setStep({ ...step, mapping })}
          onExpiry={(mapping, expiresAt) => setStep({ ...step, mapping, expiresAt })}
          history={effectiveHistory(target.type, retention)}
          type={target.type}
        />
      ) : null}
      {step.kind === "writing" ? (
        <p role="status" className="text-body">
          {t("spaces.import.writing", { sent: step.sent, total: step.total })}
        </p>
      ) : null}
      {step.kind === "done" ? <Report created={step.created} rejected={step.rejected} type={target.type} space={target.space} denied={step.denied} /> : null}
    </Dialog>
  );
}

function MapStep({
  step,
  slots,
  valid,
  refused,
  onMapping,
  onExpiry,
  history,
  type,
}: {
  step: { table: Rows; mapping: Mapping; name: string; expiresAt?: string };
  slots: ImportSlot[];
  valid: number;
  refused: Rejected[];
  onMapping: (mapping: Mapping) => void;
  /** Sets the mapping and the fixed time together, so neither change overwrites the other. */
  onExpiry: (mapping: Mapping, expiresAt: string | undefined) => void;
  history: ReturnType<typeof effectiveHistory>;
  type: string;
}): JSX.Element {
  const { t } = useTranslation();
  const { table, mapping } = step;
  const used = new Set(Object.values(mapping).filter(Boolean));
  const missing = slots.filter((slot) => slot.required && !used.has(slot.name)).map((slot) => slot.name);
  return (
    <div className="flex flex-col gap-3">
      <p className="text-body text-fg-muted">
        {t("spaces.import.read", { name: step.name, rows: table.rows.length, columns: table.columns.length })}
      </p>
      {/* A file cut at MAX_ROWS says so: the count above is not the whole file (F-UI3-011). */}
      {table.total !== undefined ? <Alert tone="warning">{t("spaces.import.cut", { total: table.total, max: MAX_ROWS })}</Alert> : null}
      {table.detected ? (
        <p className="text-caption text-fg-muted" data-testid="import-detected">
          {[
            table.detected.encoding ? t("spaces.import.detected.encoding", { encoding: table.detected.encoding.toUpperCase() }) : null,
            table.detected.separator ? t(`spaces.import.detected.separator.${table.detected.separator === "\t" ? "tab" : table.detected.separator === ";" ? "semicolon" : "comma"}`) : null,
            t(table.detected.decimalComma ? "spaces.import.detected.decimalComma" : "spaces.import.detected.decimalPoint"),
            table.detected.dateFormat ? t("spaces.import.detected.dates", { format: table.detected.dateFormat }) : null,
          ]
            .filter(Boolean)
            .join(" · ")}
        </p>
      ) : null}
      <details>
        <summary className="focus-ring cursor-pointer rounded-md text-body font-medium text-fg">
          {t("spaces.import.preview", { count: Math.min(PREVIEW_ROWS, table.rows.length) })}
        </summary>
        <Table caption={t("spaces.import.previewCaption")} maxHeight="max-h-80">
          <TableHead>
            <TableHeaderCell>#</TableHeaderCell>
            {table.columns.map((column) => (
              <TableHeaderCell key={column}>{column}</TableHeaderCell>
            ))}
          </TableHead>
          <TableBody>
            {table.rows.slice(0, PREVIEW_ROWS).map((row, at) => (
              <TableRow key={at}>
                <TableCell className="text-fg-muted">{at + 2}</TableCell>
                {table.columns.map((column) => (
                  <TableCell key={column} className="max-w-48 truncate">
                    {row[column]}
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </details>
      <Table caption={t("spaces.import.mapping")} maxHeight="max-h-80">
        <TableHead>
          <TableHeaderCell>{t("spaces.import.column")}</TableHeaderCell>
          <TableHeaderCell>{t("spaces.import.field")}</TableHeaderCell>
          <TableHeaderCell>{t("spaces.import.sample")}</TableHeaderCell>
        </TableHead>
        <TableBody>
          {table.columns.map((column, at) => {
            const id = `import-map-${at}`;
            return (
              <TableRow key={column}>
                <TableCell>
                  <label htmlFor={id} className="font-mono">
                    {column}
                  </label>
                </TableCell>
                <TableCell>
                  <Select
                    id={id}
                    // Wide enough to read the choice at 400 px; the table scrolls sideways instead.
                    className="min-w-44"
                    value={mapping[column] ?? ""}
                    onChange={(event) => onMapping({ ...mapping, [column]: event.target.value })}
                  >
                    <option value="">{t("spaces.import.skip")}</option>
                    <option value={ID} disabled={used.has(ID) && mapping[column] !== ID}>
                      {t("spaces.import.id")}
                    </option>
                    {slots.map((slot) => (
                      <option key={slot.name} value={slot.name} disabled={used.has(slot.name) && mapping[column] !== slot.name}>
                        {slot.required ? t("spaces.import.required", { name: slot.name }) : slot.name}
                      </option>
                    ))}
                    <option value={EXPIRES_AT} disabled={used.has(EXPIRES_AT) && mapping[column] !== EXPIRES_AT}>
                      {t("spaces.import.expiry.column")}
                    </option>
                  </Select>
                </TableCell>
                <TableCell className="max-w-64 truncate text-fg-muted">
                  {table.rows
                    .slice(0, PREVIEW)
                    .map((row) => row[column])
                    .filter(Boolean)
                    .join(" · ")}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      <ImportExpiry
        columns={table.columns}
        mapping={mapping}
        expiresAt={step.expiresAt}
        onChange={onExpiry}
        history={history}
        type={type}
      />
      {missing.length > 0 ? (
        <Alert tone="warning">{t("spaces.import.missing", { names: missing.join(", ") })}</Alert>
      ) : null}
      <p role="status" className="text-body">
        {t("spaces.import.checked", { valid, refused: refused.length })}
      </p>
      {refused.length > 0 ? <RefusedList rejected={refused} /> : null}
    </div>
  );
}

/** A `datetime-local` value, in the person's time zone, for an instant. */
function localInput(iso: string): string {
  const date = new Date(iso);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/**
 * When the imported entities go away (T-4822): never, at the time a column gives, or at one
 * fixed time; and what the space's history rule does with their history meanwhile (DM-97).
 */
function ImportExpiry({
  columns,
  mapping,
  expiresAt,
  onChange,
  history,
  type,
}: {
  columns: string[];
  mapping: Mapping;
  expiresAt?: string;
  onChange: (mapping: Mapping, expiresAt: string | undefined) => void;
  history: ReturnType<typeof effectiveHistory>;
  type: string;
}): JSX.Element {
  const { t } = useTranslation();
  const column = Object.keys(mapping).find((name) => mapping[name] === EXPIRES_AT);
  const mode = column !== undefined ? "column" : expiresAt !== undefined ? "fixed" : "never";
  const without = Object.fromEntries(Object.entries(mapping).filter(([, target]) => target !== EXPIRES_AT));
  const choose = (next: "never" | "column" | "fixed") => {
    onChange(
      next === "column" ? { ...without, [columns.find((name) => !mapping[name]) ?? columns[0]]: EXPIRES_AT } : without,
      next === "fixed" ? new Date(Date.now() + 86_400_000).toISOString() : undefined,
    );
  };
  const keep = history.value === undefined ? undefined : fromIso(history.value);
  const kept =
    history.value === HISTORY_OFF
      ? t("spaces.retention.offShown")
      : history.value === HISTORY_FOREVER || history.value === undefined
        ? t("spaces.retention.forever")
        : keep === undefined
          ? history.value
          : t(`spaces.retention.amount.${keep.unit}`, { count: keep.amount });
  return (
    <section aria-labelledby="import-expiry" className="flex flex-col gap-2 rounded border border-border p-3" data-testid="import-expiry">
      <h3 id="import-expiry" className="text-body font-semibold">
        {t("spaces.import.expiry.title")}
      </h3>
      <RadioGroup
        name="import-expiry-mode"
        legend={t("spaces.import.expiry.legend")}
        value={mode}
        layout="row"
        options={[
          { value: "never" as const, label: t("spaces.import.expiry.never") },
          { value: "column" as const, label: t("spaces.import.expiry.fromColumn") },
          { value: "fixed" as const, label: t("spaces.import.expiry.fixed") },
        ]}
        onChange={choose}
      />
      {mode === "column" ? (
        <Field id="import-expiry-column" label={t("spaces.import.expiry.whichColumn")} description={t("spaces.import.expiry.columnHint")}>
          <Select
            id="import-expiry-column"
            value={column}
            onChange={(event) => onChange({ ...without, [event.target.value]: EXPIRES_AT }, undefined)}
          >
            {columns.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        </Field>
      ) : null}
      {mode === "fixed" && expiresAt !== undefined ? (
        <Field id="import-expiry-at" label={t("spaces.import.expiry.at")} description={t("spaces.import.expiry.atHint")}>
          <Input
            id="import-expiry-at"
            type="datetime-local"
            min={localInput(new Date().toISOString())}
            value={localInput(expiresAt)}
            onChange={(event) => {
              const ms = Date.parse(event.target.value);
              if (!Number.isNaN(ms)) onChange(mapping, new Date(ms).toISOString());
            }}
          />
        </Field>
      ) : null}
      <p className="text-caption text-fg-muted">
        {t(`spaces.import.expiry.history.${history.layer}`, { type, kept })}{" "}
        <RetentionGuide section="8-recipe-data-imported-from-a-file" />
      </p>
    </section>
  );
}

function RefusedList({ rejected }: { rejected: Rejected[] }): JSX.Element {
  const { t } = useTranslation();
  // Focusable, so a keyboard reaches every refused row of a long list (WCAG 2.1.1).
  return (
    <ul tabIndex={0} className="focus-ring flex max-h-48 flex-col gap-1 overflow-y-auto rounded-md text-body" aria-label={t("spaces.import.refusedList")}>
      {rejected.slice(0, LISTED).map((item, at) => (
        <li key={`${item.row}-${at}`}>
          <span className="font-medium">{t("spaces.import.row", { row: item.row })}</span> {item.reason}
        </li>
      ))}
      {rejected.length > LISTED ? <li className="text-fg-muted">{t("spaces.import.more", { count: rejected.length - LISTED })}</li> : null}
    </ul>
  );
}

function Report({
  created,
  rejected,
  type,
  space,
  denied,
}: {
  created: number;
  rejected: Rejected[];
  type: string;
  space: string;
  /** The gateway's words when it refused the person the write itself. */
  denied?: string;
}): JSX.Element {
  const { t } = useTranslation();
  if (denied !== undefined) {
    // One reason for every row: the list and its download would repeat it, so the sentence says
    // what happened and who can change it.
    return (
      <Alert role="alert" tone="danger">
        {t("spaces.import.denied", { type, space, reason: denied })}
      </Alert>
    );
  }
  return (
    <div className="flex flex-col gap-3">
      <Alert role="status" tone={rejected.length === 0 ? "success" : created > 0 ? "warning" : "danger"}>
        {t("spaces.import.done", { created, refused: rejected.length })}
      </Alert>
      {rejected.length > 0 ? (
        <>
          <RefusedList rejected={rejected} />
          <Button
            variant="secondary"
            className="w-fit"
            onClick={() => download(new Blob([rejectedCsv(rejected)], { type: "text/csv" }), `${type}-refused.csv`)}
          >
            {t("spaces.import.downloadRefused")}
          </Button>
        </>
      ) : null}
    </div>
  );
}

/**
 * The type's rows as a file, from one of the space's Endpoints that serves it: CSV, Excel or
 * JSON, narrowed to the view's filter and the type's fields. Read by the gateway with the
 * person's own session, so the file holds what this person may read and no more.
 */
export function ExportLinks({
  endpoints,
  type,
  q,
  idPattern,
  attrs,
  ids,
  label,
}: {
  endpoints: Manifest[];
  type: string;
  q: string | undefined;
  /** The grid's ID-column filter, a regular expression over ids. */
  idPattern?: string;
  attrs: string[];
  /** Only these rows, by id: the grid's checked rows (T-5186). */
  ids?: readonly string[];
  /** What the links are introduced with, "Export" without it. */
  label?: string;
}): JSX.Element | null {
  const { t } = useTranslation();
  const id = useId();
  const offering = endpoints
    .map((endpoint) => {
      const spec = (endpoint.spec ?? {}) as { slug?: string };
      const served = servedRepresentations(endpoint.spec ?? {});
      const formats = EXPORTS.filter((format) => served.includes(format));
      return { name: endpoint.metadata.name, slug: spec.slug ?? "", formats };
    })
    .filter((endpoint) => endpoint.slug && endpoint.formats.length > 0);
  const [chosen, setChosen] = useState("");
  const endpoint = offering.find((e) => e.name === chosen) ?? offering[0];
  if (!endpoint) {
    return <p className="text-body text-fg-muted">{t("spaces.export.none")}</p>;
  }
  return (
    <div className="flex flex-wrap items-end gap-2">
      {offering.length > 1 ? (
        <Field id={id} label={t("spaces.export.endpoint")} className="w-fit">
          <Select id={id} value={endpoint.name} onChange={(event) => setChosen(event.target.value)}>
            {offering.map((each) => (
              <option key={each.name} value={each.name}>
                {each.name}
              </option>
            ))}
          </Select>
        </Field>
      ) : null}
      <span className="text-body text-fg-muted">{label ?? t("spaces.export.label")}</span>
      {endpoint.formats.map((format) => (
        <a
          key={format}
          className="focus-ring rounded-md border border-border px-3 py-1.5 text-body hover:bg-surface-muted"
          href={safeHref(exportUrl(endpoint.slug, format, type, q, attrs, ids, idPattern))}
          download
        >
          {t(`spaces.export.format.${format}`)}
        </a>
      ))}
    </div>
  );
}
