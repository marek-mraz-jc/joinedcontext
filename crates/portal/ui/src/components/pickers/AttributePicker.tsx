import { reasonOf } from "../../api/client";
import { useMemo } from "react";
import type { JSX } from "react";
import { useQueries } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { readModelSource } from "../../api/datamodelSource";
import { RECORD_LINK_STYLE } from "../RecordLink";
import { filterSlotsOf } from "../entities/filters";
import type { FilterSlot } from "../entities/filters";
import { Combobox } from "./Combobox";
import type { PickerOption } from "./Combobox";
import { useOrganizationModels } from "./organizationModels";

/** The LinkML ranges a KPI or an aggregate can count with. */
const NUMBER_RANGES = new Set(["integer", "float", "double", "decimal", "number"]);

export interface AttributePickerProps {
  id?: string;
  label: string;
  /** Named by the form's `<label for={id}>` (see `Combobox`). */
  labelled?: boolean;
  project: string;
  /** The context space whose model is read; absent: any model of the project that declares the type. */
  space?: string;
  /** The entity type whose attributes are offered; none chosen yet lists nothing. */
  type?: string;
  value: string[];
  onChange: (attributes: string[]) => void;
  multiple?: boolean;
  /** Properties, relationships, or both (the default). */
  kind?: "Property" | "Relationship" | "any";
  /** Only attributes whose range is a number. */
  numeric?: boolean;
  required?: boolean;
  invalid?: boolean;
  describedBy?: string;
  /** Stamped by a wrapping `Field` (see `Combobox`). */
  "aria-describedby"?: string;
}

/** Whether a slot belongs in a picker of `kind`, `numeric` narrowed to numbers. */
export function offered(slot: FilterSlot, kind: AttributePickerProps["kind"], numeric: boolean): boolean {
  if (slot.formula) return false;
  if (kind !== undefined && kind !== "any" && slot.kind !== kind) return false;
  return !numeric || NUMBER_RANGES.has((slot.range ?? "").toLowerCase());
}

/**
 * An attribute of an entity type, picked from the model that declares the type (T-4946): the
 * same models the type picker reads, so a person is never offered a name of a model they cannot
 * read. The server still checks every submitted name against the model and the Policy.
 */
export function AttributePicker({
  id,
  label,
  labelled,
  project,
  space,
  type,
  value,
  onChange,
  multiple = false,
  kind = "any",
  numeric = false,
  required,
  invalid,
  describedBy,
  "aria-describedby": fieldDescribedBy,
}: AttributePickerProps): JSX.Element {
  const { t } = useTranslation();
  const query = useOrganizationModels();
  const holders = useMemo(() => {
    const items = query.data?.items ?? [];
    if (!type) return [];
    // A list entry without its classes (an older Portal) holds none we can name.
    const holding = items.filter((m) => (m.classes ?? []).includes(type));
    const own = holding.filter((m) => m.project === project && (space === undefined || m.space === space));
    // The space's own model first; a class it imports lives in another model the caller reads.
    return own.length > 0 ? own : holding;
  }, [query.data, project, space, type]);
  const sources = useQueries({
    queries: holders.map((model) => ({
      queryKey: ["datamodels", model.project, model.name, "source"],
      queryFn: () => readModelSource(model.project, model.name),
      staleTime: 60_000,
    })),
  });

  const options = useMemo(() => {
    const seen = new Set<string>();
    const out: PickerOption[] = [];
    sources.forEach((source, at) => {
      if (!source.data || !type) return;
      for (const slot of filterSlotsOf(source.data, type)) {
        if (seen.has(slot.name) || !offered(slot, kind, numeric)) continue;
        seen.add(slot.name);
        out.push({
          value: slot.name,
          label: slot.name,
          detail: slot.range,
          group: t("picker.attribute.group", { type, model: holders[at].name }),
          badge: slot.kind === "Relationship" ? t("picker.attribute.relationship") : undefined,
        });
      }
    });
    return out;
  }, [sources, holders, type, kind, numeric, t]);

  const failedSource = sources.find((s) => s.isError);
  const failed = query.isError
    ? { reason: reasonOf(query.error), retry: () => void query.refetch() }
    : failedSource
      ? { reason: reasonOf(failedSource.error), retry: () => void failedSource.refetch() }
      : undefined;
  const loading = query.isLoading || sources.some((s) => s.isLoading);
  const empty = !type
    ? t("picker.attribute.noType")
    : numeric
      ? t("picker.attribute.noneNumeric", { type })
      : t("picker.attribute.none", { type });

  return (
    <Combobox
      id={id}
      label={label}
      labelled={labelled}
      value={value}
      multiple={multiple}
      options={options}
      disabled={!type}
      required={required}
      invalid={invalid}
      describedBy={describedBy}
      aria-describedby={fieldDescribedBy}
      loading={loading}
      failed={failed}
      empty={empty}
      onChange={onChange}
      note={
        type && !loading && !failed && options.length === 0 ? (
          <Link to="/projects/$project/models" params={{ project }} className={`${RECORD_LINK_STYLE} text-caption underline`}>
            {t("picker.attribute.addInModel")}
          </Link>
        ) : undefined
      }
    />
  );
}
