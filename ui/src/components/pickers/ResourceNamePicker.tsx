import { useMemo } from "react";
import type { JSX } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../api/client";
import { projectsQuery } from "../../api/projects";
import { asManifests, localized } from "../../api/manifest";
import { Combobox } from "./Combobox";
import type { PickerOption } from "./Combobox";

export interface ResourceNamePickerProps {
  id?: string;
  label: string;
  /** Named by a `<label for={id}>` of the page (see `Combobox`). */
  labelled?: boolean;
  /**
   * Whose manifests: a project's `{plural}`, several such lists offered as one (a project's roles
   * and the organization's), or `"projects"` for the projects the caller reads.
   */
  from: { project: string; plural: string } | { project: string; plural: string }[] | "projects";
  value: string;
  onChange: (name: string) => void;
  /** Offer the typed text as a new name, where the form creates what it names. */
  create?: boolean;
  disabled?: boolean;
  required?: boolean;
  invalid?: boolean;
  describedBy?: string;
  /** What a surrounding `Field` hands its control: its hint and error ids. */
  "aria-describedby"?: string;
  placeholder?: string;
  /** What an empty list says: where the missing thing is made. */
  empty?: string;
}

/**
 * A name of something that exists, picked from the list the server gives this caller (ADR-N-033,
 * T-2702): the list holds nothing they cannot read, so the picker offers nothing they could not open.
 */
export function ResourceNamePicker({
  id,
  label,
  labelled,
  from,
  value,
  onChange,
  create = false,
  disabled,
  required,
  invalid,
  describedBy,
  placeholder,
  "aria-describedby": fieldDescribedBy,
  empty,
}: ResourceNamePickerProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const lists = from === "projects" ? [] : Array.isArray(from) ? from : [from];
  const inProject = lists.length > 0 ? lists : undefined;
  // The projects are asked for only by a picker of projects: the key is `useProjects`' own.
  const projects = useQuery({
    ...projectsQuery,
    select: (list) => list.items.map((item) => item.name),
    enabled: !inProject,
  });
  const manifests = useQueries({
    queries: lists.map((list) => ({
      queryKey: queryKeys.list(list.project, list.plural),
      queryFn: async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/{plural}", {
            params: { path: { project: list.project, plural: list.plural } },
          }),
        ),
      enabled: Boolean(list.project && list.plural),
    })),
  });
  const failedList = manifests.find((list) => list.isError);
  const loading = inProject ? manifests.some((list) => list.isLoading) : projects.isLoading;
  const failure = inProject ? failedList : projects.isError ? projects : undefined;

  const options = useMemo<PickerOption[]>(() => {
    if (!inProject) {
      return (projects.data ?? []).map((name) => ({ value: name, label: name, group: label }));
    }
    const seen = new Set<string>();
    return manifests.flatMap((list) => asManifests(list.data?.items ?? [])).filter((manifest) => {
      // The first list wins a name both hold: a project's own role over the organization's (PF-69).
      if (seen.has(manifest.metadata.name)) return false;
      seen.add(manifest.metadata.name);
      return true;
    }).map((manifest) => {
      const title = localized(manifest.metadata.title, i18n.language, manifest.metadata.name);
      return {
        value: manifest.metadata.name,
        label: title,
        detail: title === manifest.metadata.name ? undefined : manifest.metadata.name,
        group: label,
      };
    });
  }, [inProject, projects.data, manifests, i18n.language, label]);

  return (
    <Combobox
      id={id}
      label={label}
      labelled={labelled}
      value={value ? [value] : []}
      options={options}
      disabled={disabled}
      required={required}
      invalid={invalid}
      describedBy={describedBy}
      aria-describedby={fieldDescribedBy}
      placeholder={placeholder}
      loading={loading}
      failed={failure ? { reason: reasonOf(failure.error), retry: () => void failure.refetch() } : undefined}
      empty={empty ?? t("form.noResults")}
      onChange={(values) => {
        onChange(values[0] ?? "");
      }}
      create={create ? { label: (name) => t("picker.createNamed", { name }), onCreate: onChange } : undefined}
    />
  );
}
