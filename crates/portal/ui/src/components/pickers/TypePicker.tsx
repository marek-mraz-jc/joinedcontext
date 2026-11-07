import { reasonOf } from "../../api/client";
import { useMemo } from "react";
import type { JSX } from "react";
import { useQueries } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parse } from "yaml";
import { readModelSource } from "../../api/datamodelSource";
import { Combobox } from "./Combobox";
import type { PickerOption } from "./Combobox";
import { useOrganizationModels } from "./organizationModels";
import type { OrganizationModel } from "./organizationModels";

export interface TypePickerProps {
  id?: string;
  label: string;
  /** Named by the form's `<label for={id}>` (see `Combobox`). */
  labelled?: boolean;
  /** The project whose types are offered. */
  project: string;
  /** The context space: its model's classes and the classes it imports. Absent: every model of the project. */
  space?: string;
  value: string[];
  onChange: (types: string[]) => void;
  multiple?: boolean;
  /** Offered when nothing matches: a type the model does not hold yet. */
  onCreate?: (type: string) => void;
  disabled?: boolean;
  required?: boolean;
  invalid?: boolean;
  describedBy?: string;
  /** What a surrounding `Field` hands its control: its hint and error ids. */
  "aria-describedby"?: string;
}

/** A platform-model import (DM-76): `org.{name}.v{major}` or `project.{name}.v{major}`. */
const MODEL_IMPORT = /^(org|project)\.([a-z0-9](?:[-a-z0-9]*[a-z0-9])?)\.v(?:0|[1-9][0-9]*)$/;

/** A model one source imports: by its platform name with its level, or by a file or URL's name. */
export interface ImportedModel {
  name: string;
  level?: "organization" | "project";
}

/**
 * The models a LinkML source imports: `org.`/`project.` names with their level (DM-76), and the
 * last segment of any other file or URL import. LinkML's own `linkml:` imports and a source that
 * does not parse import nothing here.
 */
export function importedNames(source: string): ImportedModel[] {
  let doc: unknown;
  try {
    doc = parse(source);
  } catch {
    return [];
  }
  const imports = (doc as { imports?: unknown } | null)?.imports;
  if (!Array.isArray(imports)) {
    return [];
  }
  return imports
    .filter((entry): entry is string => typeof entry === "string" && !entry.startsWith("linkml:"))
    .map((entry): ImportedModel => {
      const platform = MODEL_IMPORT.exec(entry);
      if (platform) {
        return { name: platform[2], level: platform[1] === "org" ? "organization" : "project" };
      }
      return { name: (entry.split(/[/#]/).pop() ?? "").replace(/\.linkml\.ya?ml$|\.ya?ml$/, "") };
    })
    .filter((imported) => imported.name !== "");
}

/**
 * The entity types a form may name (ADR-N-033): the classes of the space's model first, then the
 * classes it imports, each grouped under the model that declares it. Every class comes from a model
 * the server listed for this caller, so the picker offers no type of a model they cannot read.
 */
export function TypePicker({
  id,
  label,
  labelled,
  project,
  space,
  value,
  onChange,
  multiple = false,
  onCreate,
  disabled,
  required,
  invalid,
  describedBy,
  "aria-describedby": fieldDescribedBy,
}: TypePickerProps): JSX.Element {
  const { t } = useTranslation();
  const query = useOrganizationModels();
  const models = useMemo(() => query.data?.items ?? [], [query.data]);
  const local = useMemo(
    () => models.filter((m) => m.project === project && (space === undefined || m.space === space)),
    [models, project, space],
  );

  // Only a space's own model says what it imports; without a space every model is listed anyway.
  const sources = useQueries({
    queries: (space === undefined ? [] : local).map((model) => ({
      queryKey: ["datamodels", model.project, model.name, "source"],
      queryFn: () => readModelSource(model.project, model.name),
      staleTime: 60_000,
    })),
  });

  const options = useMemo(() => {
    const seen = new Set<string>();
    const out: PickerOption[] = [];
    const add = (model: OrganizationModel, group: string) => {
      for (const cls of model.classes) {
        if (seen.has(cls)) continue;
        seen.add(cls);
        out.push({ value: cls, label: cls, detail: `${model.project}/${model.name}`, group, badge: `v${model.version}` });
      }
    };
    for (const model of local) {
      add(
        model,
        space !== undefined
          ? t("picker.type.local", { space })
          : model.space
            ? t("picker.type.modelGroup", { name: model.name, space: model.space })
            : t("picker.type.modelGroupOwn", { name: model.name }),
      );
    }
    const imported = sources.flatMap((s) => (s.data ? importedNames(s.data) : []));
    for (const { name, level } of imported) {
      const model =
        level === "organization"
          ? models.find((m) => m.name === name && m.level === "organization")
          : level === "project"
            ? models.find((m) => m.name === name && m.project === project)
            : // A file or URL import names no organization model: those have a platform name.
              (models.find((m) => m.name === name && m.project === project) ??
              models.find((m) => m.name === name && m.level !== "organization"));
      if (model && !local.includes(model)) {
        add(
          model,
          model.level === "organization"
            ? t("picker.type.importedOrganization", { name: model.name })
            : t("picker.type.imported", { name: `${model.project}/${model.name}` }),
        );
      }
    }
    return out;
  }, [local, models, project, space, sources, t]);

  const failedSource = sources.find((s) => s.isError);
  const failed = query.isError
    ? { reason: reasonOf(query.error), retry: () => void query.refetch() }
    : failedSource
      ? { reason: reasonOf(failedSource.error), retry: () => void failedSource.refetch() }
      : undefined;

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
      loading={query.isLoading || sources.some((s) => s.isLoading)}
      failed={failed}
      empty={space === undefined ? t("picker.type.noneInProject") : t("picker.type.none", { space })}
      onChange={onChange}
      create={onCreate ? { label: (name) => t("picker.type.create", { name }), onCreate } : undefined}
    />
  );
}
