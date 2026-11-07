import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listSpaceTypes } from "../../api/gateway";
import { clsx } from "clsx";
import { localized } from "../../api/manifest";
import { Button, Checkbox, Field, Input, Select } from "../ui";
import { ToolbarPopover } from "./GridToolbar";
import { filtersFromQ, opsFor, qFromFilters, qProblem, typeGroups } from "./filters";
import type { EntityQuery, Filter, FilterSlot } from "./filters";

export interface EntityFiltersProps {
  /** The prefix of the control ids: `${id}-type`, `${id}-q`, `${id}-scopeQ`. */
  id: string;
  /** The entity types the space's DataModel declares; none: the types the space's broker lists. */
  types: string[];
  /** The context space read, so a space whose model declares no type still offers the types it holds. */
  space?: string;
  /** The attributes of the chosen type, typed by the model (UI-33); may be empty. */
  slots: FilterSlot[];
  value: EntityQuery;
  onChange: (next: EntityQuery) => void;
  /** Attributes the caller may not read, by name, with the reason (T-0529). */
  denied?: Record<string, string>;
  /**
   * The types the source grants this person a read of (T-3218), offered first; the others follow,
   * each saying it is not granted, so nobody picks a type to meet an empty grid. Unknown or every
   * type: leave it out.
   */
  readable?: string[];
}

/**
 * The filters of one entity query, generated from the model (UI-33): the type, one row per
 * filter with the operators and input the slot's range allows, `scopeQ`, and the attributes to
 * read. Every row is a view of `value.q`; a `q` the rows cannot show stays a text field.
 */
export function EntityFilters(props: EntityFiltersProps): JSX.Element {
  return props.space && props.types.length === 0 ? <HeldTypeFilters {...props} space={props.space} /> : <Filters {...props} />;
}

/** No model types: the broker's own list, as the gateway lets this person read it (T-4946). */
function HeldTypeFilters(props: EntityFiltersProps & { space: string }): JSX.Element {
  const held = useQuery({
    queryKey: ["spaceTypes", props.space],
    queryFn: ({ signal }) => listSpaceTypes(props.space, signal),
    staleTime: 60_000,
  });
  const types = (held.data ?? []).map((entry) => entry.type);
  const slots: FilterSlot[] =
    props.slots.length > 0
      ? props.slots
      : (held.data?.find((entry) => entry.type === props.value.type)?.attributes ?? []).map((name) => ({ name, kind: "Property" }));
  return <Filters {...props} types={types} slots={slots} loading={held.isLoading} />;
}

function Filters({
  id,
  types,
  slots,
  value,
  onChange,
  denied = {},
  readable,
  loading = false,
}: EntityFiltersProps & { loading?: boolean }): JSX.Element {
  const { t, i18n } = useTranslation();
  const q = value.q ?? "";
  /** What a reader calls an attribute: the model's label, the name itself without one. */
  const labelOf = (name: string) => localized(slots.find((slot) => slot.name === name)?.label, i18n.language, name);
  // The rows are derived from `q`, but a row being typed (no value yet) is not in `q`, so they
  // live here and are re-derived only when `q` changed under them (the YAML view, a reset).
  const [state, setState] = useState(() => ({ q, rows: filtersFromQ(q) }));
  if (q !== state.q) {
    setState({ q, rows: filtersFromQ(q) });
  }
  const rows = q === state.q ? state.rows : filtersFromQ(q);
  const problem = qProblem(q);

  function setRows(next: Filter[]) {
    const composed = qFromFilters(next, slots);
    setState({ q: composed, rows: next });
    onChange({ ...value, q: composed || undefined });
  }

  function setRow(index: number, patch: Partial<Filter>) {
    setRows(rows!.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  }

  function toggleAttribute(attribute: string, on: boolean) {
    const current = value.attrs ?? [];
    const attrs = on ? [...current, attribute] : current.filter((a) => a !== attribute);
    onChange({ ...value, attrs: attrs.length > 0 ? attrs : undefined });
  }

  return (
    <div className="flex flex-col gap-3" data-testid={`${id}-filters`}>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field
          id={`${id}-type`}
          label={t("entities.type")}
          description={types.length === 0 && !loading ? t("entities.noTypes") : t("entities.hint.type")}
        >
          <TypeSelect
            id={`${id}-type`}
            types={types}
            readable={readable}
            value={value}
            onChange={(type) => onChange({ type, scopeQ: value.scopeQ })}
          />
        </Field>
        <Field id={`${id}-scopeQ`} label={t("entities.scopeQ")} description={t("entities.hint.scope")}>
          <Input
            id={`${id}-scopeQ`}
            value={value.scopeQ ?? ""}
            onChange={(event) => onChange({ ...value, scopeQ: event.target.value || undefined })}
          />
        </Field>
      </div>

      {value.type ? (
        <fieldset className="flex flex-col gap-2">
          <legend className="mb-1 text-caption text-fg-muted">{t("entities.filters")}</legend>
          <p className="text-caption text-fg-muted">{t("entities.hint.filters")}</p>
          {rows ? (
            rows.map((row, index) => {
              const slot = slots.find((s) => s.name === row.attribute);
              return (
                <div key={index} className="flex flex-wrap items-center gap-2">
                  <Select
                    aria-label={t("entities.attribute")}
                    value={row.attribute}
                    disabled={slots.length === 0}
                    title={slots.length === 0 ? t("entities.noAttributes") : undefined}
                    onChange={(event) => setRow(index, { attribute: event.target.value, op: "==", value: "" })}
                  >
                    <option value="">—</option>
                    {/* A name `q` holds that the model does not list stays visible, never silently dropped. */}
                    {[...new Set([...(row.attribute && !slots.some((s) => s.name === row.attribute) ? [row.attribute] : []), ...slots.map((s) => s.name)])].map(
                      (name) => (
                        <option key={name} value={name}>
                          {labelOf(name) === name ? name : `${labelOf(name)} · ${name}`}
                        </option>
                      ),
                    )}
                  </Select>
                  <Select
                    aria-label={t("entities.operator")}
                    value={row.op}
                    className="w-20"
                    onChange={(event) => setRow(index, { op: event.target.value as Filter["op"] })}
                  >
                    {opsFor(slot).map((op) => (
                      <option key={op} value={op}>
                        {op}
                      </option>
                    ))}
                  </Select>
                  <ValueInput slot={slot} value={row.value} onChange={(next) => setRow(index, { value: next })} />
                  <Button size="sm" variant="ghost" onClick={() => setRows(rows.filter((_, i) => i !== index))}>
                    {t("entities.remove")}
                  </Button>
                </div>
              );
            })
          ) : /[|()]/.test(q) ? (
            <p className="text-caption text-fg-muted">{t("entities.advanced")}</p>
          ) : null}
          {rows ? (
            <div>
              <Button
                size="sm"
                variant="secondary"
                onClick={() => setRows([...rows, { attribute: slots[0]?.name ?? "", op: "==", value: "" }])}
              >
                {t("entities.addFilter")}
              </Button>
            </div>
          ) : null}
          {/* The query language stays out of sight until someone asks for it, or until it says
              something the rows cannot (T-5622). */}
          <details
            className="rounded border border-border p-2"
            data-testid={`${id}-technical`}
            open={Boolean(problem) || rows === undefined || undefined}
          >
            <summary className="cursor-pointer text-caption text-fg-muted">{t("entities.technical")}</summary>
            <p className="mt-2 text-caption text-fg-muted">{t("entities.technicalScope")}</p>
          <Field
            id={`${id}-q`}
            label={t("entities.q")}
            description={t("entities.hint.q")}
            errors={
              problem
                ? [problem.kind === "unquoted" ? t("entities.qProblem.unquoted", { fix: problem.fix }) : t("entities.qProblem.shape")]
                : undefined
            }
          >
            <Input
              id={`${id}-q`}
              value={q}
              className="font-mono"
              onChange={(event) => onChange({ ...value, q: event.target.value || undefined })}
            />
          </Field>
          </details>
        </fieldset>
      ) : null}

      {value.type && slots.length > 0 ? (
        // A Columns menu, not a wall of checkboxes above the data (T-6092): the model's label
        // reads alone, the name the data carries is its tooltip.
        <div>
          <ToolbarPopover label={t("entities.columns")} count={(value.attrs ?? []).length} outlined>
            <fieldset className="flex flex-col gap-2">
              <legend className="mb-1 text-caption text-fg-muted">{t("entities.attrs")}</legend>
              <p className="text-caption text-fg-muted">{t("entities.hint.attrs")}</p>
              {/* The shared `Checkbox`, not a hand-written label around a bare input: a refused
                  attribute keeps its place in the tab order and says why (T-1756, UI-44). */}
              {slots.map((slot) => (
                <Checkbox
                  key={slot.name}
                  title={slot.name}
                  label={labelOf(slot.name) === slot.name ? <span className="font-mono">{slot.name}</span> : labelOf(slot.name)}
                  className={clsx("gap-1 text-caption", denied[slot.name] && "text-fg-muted line-through")}
                  disabled={Boolean(denied[slot.name])}
                  disabledReason={denied[slot.name]}
                  checked={(value.attrs ?? []).includes(slot.name)}
                  onChange={(event) => toggleAttribute(slot.name, event.target.checked)}
                />
              ))}
            </fieldset>
          </ToolbarPopover>
        </div>
      ) : null}
    </div>
  );
}

/** The type picker: what the source grants first, the rest in a group that says so (T-3218, T-5622). */
function TypeSelect({
  id,
  types,
  readable,
  value,
  onChange,
}: {
  id: string;
  types: string[];
  readable: string[] | undefined;
  value: EntityQuery;
  onChange: (type: string | undefined) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const { granted, ungranted } = typeGroups(types, value.type, readable);
  const none = types.length === 0 && !value.type;
  return (
    <Select
      id={id}
      value={value.type ?? ""}
      disabled={none}
      title={none ? t("entities.noTypes") : undefined}
      onChange={(event) => onChange(event.target.value || undefined)}
    >
      <option value="">—</option>
      {granted.map((name) => (
        <option key={name} value={name}>
          {name}
        </option>
      ))}
      {ungranted.length > 0 ? (
        <optgroup label={t("entities.typesNotGranted")}>
          {ungranted.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </optgroup>
      ) : null}
    </Select>
  );
}

const NUMERIC = ["integer", "float", "double", "decimal"];

/** The input the slot's range calls for: a number with its bounds, a choice, a URN, or text. */
function ValueInput({
  slot,
  value,
  onChange,
}: {
  slot: FilterSlot | undefined;
  value: string;
  onChange: (next: string) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const label = t("entities.value");
  if (slot?.range === "boolean") {
    return (
      <Select aria-label={label} value={value} className="w-28" onChange={(event) => onChange(event.target.value)}>
        <option value="">—</option>
        <option value="true">true</option>
        <option value="false">false</option>
      </Select>
    );
  }
  if (slot?.values) {
    return (
      <Select aria-label={label} value={value} className="w-40" onChange={(event) => onChange(event.target.value)}>
        <option value="">—</option>
        {slot.values.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </Select>
    );
  }
  if (NUMERIC.includes(slot?.range ?? "")) {
    return (
      <Input
        aria-label={label}
        type="number"
        value={value}
        min={slot?.minimum}
        max={slot?.maximum}
        step={slot?.range === "integer" ? 1 : "any"}
        className="w-32"
        onChange={(event) => onChange(event.target.value)}
      />
    );
  }
  const placeholder =
    slot?.kind === "Relationship"
      ? "urn:ngsi-ld:…"
      : slot?.range === "date"
        ? "2026-01-31"
        : slot?.range === "datetime"
          ? "2026-01-31T10:00:00Z"
          : undefined;
  return (
    <Input
      aria-label={label}
      value={value}
      placeholder={placeholder}
      className="w-48"
      onChange={(event) => onChange(event.target.value)}
    />
  );
}
