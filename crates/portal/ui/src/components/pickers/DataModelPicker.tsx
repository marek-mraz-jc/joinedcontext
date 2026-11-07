import { reasonOf } from "../../api/client";
import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { Combobox } from "./Combobox";
import type { PickerOption } from "./Combobox";
import { catalogueValue, modelValue, useDebounced, useOrganizationModels } from "./organizationModels";
import type { CatalogueEntry, OrganizationModel } from "./organizationModels";

/** What a person picked: a model of the organization, or a Smart Data Models catalogue entry. */
export type ModelChoice = { source: "organization"; model: OrganizationModel } | { source: "catalogue"; entry: CatalogueEntry };

export interface DataModelPickerProps {
  id?: string;
  label: string;
  /** Named by the form's `<label for={id}>` (see `Combobox`). */
  labelled?: boolean;
  /** Chosen values, `project/name` or `sdm:{catalogue id}` (see `modelValue`, `catalogueValue`). */
  value: string[];
  onChange: (values: string[], choices: ModelChoice[]) => void;
  /** The form's project: its own models are listed first. */
  project?: string;
  /** List the models of this project alone. */
  only?: string;
  multiple?: boolean;
  /** Offer Smart Data Models catalogue entries as well (a search of two characters or more). */
  catalogue?: boolean;
  /** Offered when nothing matches: start a new model with the typed name. */
  onCreate?: (name: string) => void;
  disabled?: boolean;
  required?: boolean;
  invalid?: boolean;
  describedBy?: string;
  /** The wrapping Field's hint and error ids, kept beside `describedBy`. */
  "aria-describedby"?: string;
}

/**
 * Every data model the person may read, grouped by level, project and space with the version, and
 * the Smart Data Models entries a search matches (ADR-N-033, DM-63, DM-79). The list is the
 * server's, which holds nothing the caller cannot read.
 */
export function DataModelPicker({
  id,
  label,
  labelled,
  value,
  onChange,
  project,
  only,
  multiple = false,
  catalogue = true,
  onCreate,
  disabled,
  required,
  invalid,
  describedBy,
  "aria-describedby": fieldDescribedBy,
}: DataModelPickerProps): JSX.Element {
  const { t } = useTranslation();
  const [typed, setTyped] = useState("");
  const search = useDebounced(catalogue ? typed : "");
  const query = useOrganizationModels(search);
  const data = query.data;

  const { options, choices } = useMemo(() => {
    // The form's own project first, then the organization's models, then every other project.
    const rank = (m: OrganizationModel) => (m.project === project ? 0 : m.level === "organization" ? 1 : 2);
    const models = (data?.items ?? [])
      .filter((m) => only === undefined || m.project === only)
      .sort((a, b) => rank(a) - rank(b));
    const entries = catalogue ? (data?.smartDataModels ?? []) : [];
    const choices = new Map<string, ModelChoice>();
    const options: PickerOption[] = [];
    for (const model of models) {
      const value = modelValue(model);
      choices.set(value, { source: "organization", model });
      options.push({
        value,
        label: model.name,
        detail: model.classes.join(", "),
        group:
          model.level === "organization"
            ? t("picker.model.groupOrganization")
            : model.space
              ? t("picker.model.group", { project: model.project, space: model.space })
              : t("picker.model.groupProject", { project: model.project }),
        badge:
          model.lifecycle === "published"
            ? `v${model.version}`
            : `v${model.version} · ${t(`picker.model.lifecycle.${model.lifecycle}`, { defaultValue: model.lifecycle })}`,
      });
    }
    for (const entry of entries) {
      const value = catalogueValue(entry);
      choices.set(value, { source: "catalogue", entry });
      options.push({
        value,
        label: entry.name,
        detail: entry.description ?? entry.subject,
        group: t("picker.model.catalogue"),
      });
    }
    return { options, choices };
  }, [data, project, only, catalogue, t]);

  return (
    <Combobox
      id={id}
      label={label}
      labelled={labelled}
      value={value}
      multiple={multiple}
      options={options}
      disabled={disabled}
      required={required}
      invalid={invalid}
      describedBy={describedBy}
      aria-describedby={fieldDescribedBy}
      loading={query.isLoading}
      failed={query.isError ? { reason: reasonOf(query.error), retry: () => void query.refetch() } : undefined}
      empty={t("picker.model.none")}
      onSearch={setTyped}
      onChange={(values) => {
        onChange(
          values,
          values.flatMap((v) => {
            const choice = choices.get(v);
            return choice ? [choice] : [];
          }),
        );
      }}
      create={onCreate ? { label: (name) => t("picker.model.create", { name }), onCreate } : undefined}
      note={
        catalogue && data?.catalogueUnavailable
          ? t("picker.model.catalogueUnavailable", { reason: data.catalogueUnavailable })
          : catalogue && typed.trim().length < 2
            ? t("picker.model.catalogueHint")
            : undefined
      }
    />
  );
}
