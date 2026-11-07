import { useContext, useId } from "react";
import type { JSX } from "react";
import type { FieldProps } from "@rjsf/utils";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../../api/client";
import { asManifests } from "../../../api/manifest";
import { filterSlotsOf } from "../../entities/filters";
import {
  Button,
  Checkbox,
  Select,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableRowHeaderCell,
} from "../../ui";
import { FormDataContext } from "./EntitySelectorField";
import { FormProjectContext } from "./ModelWidgets";
import {
  columnBlocked,
  grantLimits,
  matrixRows,
  METADATA_MEMBERS,
  toggled,
  valueBlocked,
  VERSIONS_CAPABILITIES,
  withCapability,
  WRITE_CAPABILITIES,
} from "./attributeCapabilities";
import type { AttributeCapabilities, Capabilities, CapabilityField, GrantLimits } from "./attributeCapabilities";

type Root = ({ information?: Entry[] } & Parameters<typeof grantLimits>[0]) | undefined;

const COLUMNS: CapabilityField[] = ["read", "history", "versions", "metadata", "write"];

interface Entry {
  entities?: { type?: unknown }[];
  propertyNames?: unknown;
  relationshipNames?: unknown;
}

const strings = (value: unknown): string[] =>
  Array.isArray(value) ? value.filter((v): v is string => typeof v === "string" && v !== "") : [];

/** The rows, held block and grant limits of the `information` entry at `path`. */
function useEntryMatrix(path: readonly (string | number)[], formData: unknown) {
  const root = useContext(FormDataContext) as Root;
  const project = useContext(FormProjectContext) ?? "";
  const at = path.findIndex((step) => step === "information");
  const index = typeof path[at + 1] === "number" ? (path[at + 1] as number) : 0;
  const entry = root?.information?.[index];
  const types = [...new Set(strings((entry?.entities ?? []).map((selector) => selector?.type)))];
  const listed = [...strings(entry?.propertyNames), ...strings(entry?.relationshipNames)];
  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "datamodels" } } })),
    enabled: Boolean(project) && types.length > 0 && listed.length === 0,
  });
  const declared = asManifests(models.data?.items ?? []).flatMap((model) =>
    types.flatMap((type) => filterSlotsOf(model, type).map((slot) => slot.name)),
  );
  const caps = (formData ?? undefined) as Capabilities | undefined;
  return {
    types,
    caps,
    rows: matrixRows(listed, [...new Set(declared)].sort(), Object.keys(caps ?? {})),
    limits: grantLimits(root),
  };
}

/**
 * A Policy entry's `attributes` block as a table (ADR-N-066 2.6, DM-153): a row per attribute the
 * model declares for the entry's types, plus `*`; columns read, history, versions, metadata and
 * edit. What the grant cannot give is said once above the table and its cells are disabled. The
 * server checks every key and value again at the dry run (DM-148, DM-151).
 */
export function AttributeCapabilitiesField(props: FieldProps): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const { types, caps, rows, limits } = useEntryMatrix(props.fieldPathId.path, props.formData);
  const frozen = Boolean(props.disabled || props.readonly);
  const hints = `${id}-hints`;
  // Edit on a public grant, which can never give it, is left out unless a row sets it; the note says why.
  const columns = COLUMNS.filter(
    (column) =>
      columnBlocked(column, limits) !== "policies.capabilities.neverPublicWrite" ||
      rows.some((row) => caps?.[row]?.[column] !== undefined),
  );
  return (
    <fieldset className="flex min-w-0 flex-col gap-3">
      <legend className="text-body font-medium text-fg">{t("policies.capabilities.title")}</legend>
      <p id={hints} className="text-caption text-fg-muted">
        {t("policies.capabilities.about")}
      </p>
      <CapabilityHints />
      <GrantNote limits={limits} />
      {types.length === 0 ? <p className="text-caption text-fg-muted">{t("policies.capabilities.noType")}</p> : null}
      <Table stack caption={t("policies.capabilities.caption", { types: types.join(", ") || "—" })} zebra={false}>
        <TableHead>
          <TableHeaderCell>{t("policies.capabilities.column.attribute")}</TableHeaderCell>
          {columns.map((column) => (
            <TableHeaderCell key={column}>{t(`policies.capabilities.column.${column}`)}</TableHeaderCell>
          ))}
        </TableHead>
        <TableBody>
          {rows.map((attribute) => (
            <TableRow key={attribute}>
              <TableRowHeaderCell className="font-mono">
                {attribute === "*" ? t("policies.capabilities.every") : attribute}
              </TableRowHeaderCell>
              {columns.map((column) => (
                <TableCell key={column} data-label={t(`policies.capabilities.column.${column}`)}>
                  <CapabilityCell
                    attribute={attribute}
                    column={column}
                    row={caps?.[attribute]}
                    every={attribute === "*" ? undefined : caps?.["*"]}
                    limits={limits}
                    frozen={frozen}
                    describedBy={hints}
                    onChange={(value) =>
                      props.onChange(withCapability(caps, attribute, column, value), props.fieldPathId.path)
                    }
                  />
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </fieldset>
  );
}

/** What each column means, with an example, folded so the table stays in view (DM-153). */
export function CapabilityHints(): JSX.Element {
  const { t } = useTranslation();
  return (
    <details className="rounded-md border border-border bg-surface-subtle px-3 py-2">
      <summary className="focus-ring cursor-pointer rounded-sm text-caption font-medium text-fg">
        {t("policies.capabilities.hintsTitle")}
      </summary>
      <dl className="mt-2 grid gap-1 text-caption sm:grid-cols-[auto_minmax(0,1fr)] sm:gap-x-3">
        {COLUMNS.map((column) => (
          <div key={column} className="contents">
            <dt className="font-medium text-fg">{t(`policies.capabilities.column.${column}`)}</dt>
            <dd className="text-fg-muted">{t(`policies.capabilities.hint.${column}`)}</dd>
          </div>
        ))}
      </dl>
    </details>
  );
}

/** What this grant cannot give, once each, so no cell has to repeat it. Nothing when it can give all. */
export function GrantNote({ limits }: { limits: GrantLimits }): JSX.Element | null {
  const { t } = useTranslation();
  const reasons = [...new Set(COLUMNS.flatMap((column) => columnReasons(column, limits)))];
  if (reasons.length === 0) return null;
  return (
    <div className="rounded-md border border-border px-3 py-2 text-caption text-fg">
      <p className="font-medium">{t("policies.capabilities.limitsTitle")}</p>
      <ul className="list-disc pl-5 text-fg-muted">
        {reasons.map((reason) => (
          <li key={reason}>{t(reason)}</li>
        ))}
      </ul>
    </div>
  );
}

/** Every reason a value of `column` cannot be set under `limits`, once each, as locale keys. */
export function columnReasons(column: CapabilityField, limits: GrantLimits): string[] {
  const values: readonly string[] =
    column === "versions" ? VERSIONS_CAPABILITIES : column === "write" ? WRITE_CAPABILITIES : column === "metadata" ? METADATA_MEMBERS : [""];
  return [
    ...new Set(
      values.map((value) => valueBlocked(column, value, limits)).filter((reason): reason is string => reason !== undefined),
    ),
  ];
}

interface CellProps {
  attribute: string;
  column: CapabilityField;
  row: AttributeCapabilities | undefined;
  /** The `*` row, whose value an unset cell of another row takes. */
  every?: AttributeCapabilities;
  limits: GrantLimits;
  frozen: boolean;
  describedBy: string;
  onChange: (value: AttributeCapabilities[CapabilityField] | undefined) => void;
}

/** One cell: a dash with its reason where the grant cannot give the column, else its control. */
export function CapabilityCell(props: CellProps): JSX.Element {
  const { t } = useTranslation();
  const blocked = columnBlocked(props.column, props.limits);
  // A value the manifest holds in a blocked column stays visible, so the dry run's refusal names
  // something the person can see.
  if (blocked && props.row?.[props.column] === undefined) {
    return (
      <span className="text-fg-muted" title={t(blocked)}>
        <span aria-hidden="true">—</span>
        <span className="sr-only">{t(blocked)}</span>
      </span>
    );
  }
  if (props.column === "read" || props.column === "history") return <ChoiceCell {...props} />;
  if (props.column === "metadata") return <MetadataCell {...props} />;
  return <ListCell {...props} />;
}

/** The cell's name for a screen reader: "pm10: History". */
function useCellName(attribute: string, column: CapabilityField): string {
  const { t } = useTranslation();
  return `${attribute === "*" ? t("policies.capabilities.every") : attribute}: ${t(`policies.capabilities.column.${column}`)}`;
}

/** What an unset cell means: the grant's default on `*`, else the `*` row's value. */
function useInherited(attribute: string, inherited: unknown): string {
  const { t } = useTranslation();
  if (attribute === "*") return t("policies.capabilities.asGrant");
  if (typeof inherited === "boolean") {
    return `${t("policies.capabilities.asEvery")}: ${t(inherited ? "policies.capabilities.yes" : "policies.capabilities.no")}`;
  }
  return t("policies.capabilities.asEvery");
}

/** Read or history: inherit, yes or no. */
export function ChoiceCell({ attribute, column, row, every, limits, frozen, describedBy, onChange }: CellProps): JSX.Element {
  const { t } = useTranslation();
  const value = row?.[column];
  const name = useCellName(attribute, column);
  const inherit = useInherited(attribute, every?.[column]);
  // A stacked cell (phone) aligns its items to the start: the wrapper stretches the select across.
  return (
    <div className="w-full justify-self-stretch">
    <Select
      aria-label={name}
      aria-describedby={describedBy}
      className="w-full min-w-32"
      disabled={frozen || (columnBlocked(column, limits) !== undefined && value !== true)}
      value={value === undefined ? "" : String(value)}
      onChange={(event) => onChange(event.target.value === "" ? undefined : event.target.value === "true")}
    >
      <option value="">{inherit}</option>
      <option value="true">{t("policies.capabilities.yes")}</option>
      <option value="false">{t("policies.capabilities.no")}</option>
    </Select>
    </div>
  );
}

/** The ticks of a list column, and a control back to the inherited value once one is set. */
export function ListCell({ attribute, column, row, every, limits, frozen, onChange }: CellProps): JSX.Element {
  const { t } = useTranslation();
  const name = useCellName(attribute, column);
  const vocabulary: readonly string[] =
    column === "versions" ? VERSIONS_CAPABILITIES : column === "write" ? WRITE_CAPABILITIES : METADATA_MEMBERS;
  const list = row?.[column] as string[] | undefined;
  const inherit = useInherited(attribute, every?.[column]);
  // Versions and edit values read as words; metadata members are names a reader looks up as such.
  const word = (item: string): string => (column === "metadata" ? item : t(`policies.capabilities.value.${item}`));
  return (
    <div className="flex flex-col gap-1">
      {vocabulary.map((item) => {
        const reason = valueBlocked(column, item, limits);
        const ticked = list?.includes(item) ?? false;
        return (
          <Checkbox
            key={item}
            label={<span className={column === "metadata" ? "font-mono text-caption" : undefined}>{word(item)}</span>}
            aria-label={`${name} ${item}`}
            checked={ticked}
            disabled={frozen || (reason !== undefined && !ticked)}
            disabledReason={reason ? t(reason) : undefined}
            onChange={(event) => onChange(toggled(vocabulary, list, item, event.target.checked) as AttributeCapabilities[CapabilityField])}
          />
        );
      })}
      {list !== undefined && !frozen ? (
        <Button size="xs" variant="ghost" className="self-start" aria-label={`${name}: ${inherit}`} onClick={() => onChange(undefined)}>
          {inherit}
        </Button>
      ) : (
        <span className="text-caption text-fg-muted">{list === undefined ? inherit : null}</span>
      )}
    </div>
  );
}

/** Nine members: folded behind a summary that names the chosen ones, so a row stays short. */
export function MetadataCell(props: CellProps): JSX.Element {
  const list = props.row?.metadata;
  const inherit = useInherited(props.attribute, undefined);
  const { t } = useTranslation();
  return (
    <details className="min-w-40">
      <summary className="focus-ring cursor-pointer rounded-sm text-caption text-fg">
        {list === undefined ? inherit : list.length === 0 ? t("policies.capabilities.none") : list.join(", ")}
      </summary>
      <div className="mt-2">
        <ListCell {...props} />
      </div>
    </details>
  );
}
