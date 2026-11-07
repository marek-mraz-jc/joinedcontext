import { useContext, useMemo } from "react";
import type { ChangeEvent, FocusEvent, JSX } from "react";
import { ariaDescribedByIds } from "@rjsf/utils";
import type { WidgetProps } from "@rjsf/utils";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../../api/client";
import { asManifests, localized } from "../../../api/manifest";
import { Button, InlineError, Select } from "../../ui";
import { useShownErrors } from "../touched";
import { FormProjectContext } from "./ModelWidgets";

/**
 * A parameter whose choices are the project's own manifests of one kind (CC-24).
 *
 * The schema names the plural segment of `/api/v1/projects/{project}/{plural}`; the chosen
 * manifest's name is the value. The list comes back already narrowed to what the caller may
 * read, so the picker shows nobody an object they could not open themselves, and the server
 * validates the value against the same schema either way.
 */
export function ResourcePicker(props: WidgetProps): JSX.Element {
  const { id, value, required, disabled, readonly, onChange, onBlur, onFocus, options, rawErrors } =
    props;
  const { t, i18n } = useTranslation();
  const errors = useShownErrors(id, rawErrors);
  const invalid = errors && errors.length > 0 ? "true" : undefined;

  const plural = typeof options?.plural === "string" ? options.plural : undefined;
  // Written into the uiSchema by the page that renders the form: a widget has no route of
  // its own, and a parameter schema must not be able to name another project.
  const formProject = useContext(FormProjectContext);
  const project = (typeof options?.project === "string" ? options.project : undefined) ?? formProject;

  const { query, choices } = useResourceChoices(project, plural, i18n.language);

  // A list that could not be asked for is not an empty project (UI-44, T-1486): a 403 or a 500
  // read as "No results" in every form that picks a resource, so nobody could tell the difference.
  const failed = query.isError;
  const reason = reasonOf(query.error);
  const empty = placeholder(t, query.isLoading, failed, choices.length);

  // What an empty list asks the person to do next (T-5590): the page's own words, else the
  // generic next step, never a bare "No results" (T-6163).
  const nothing = query.isSuccess && choices.length === 0;
  const emptyText = typeof options?.empty === "string" ? options.empty : "form.noneYet";

  const select = (
    <Select
      id={id}
      required={required}
      disabled={disabled || readonly}
      aria-describedby={nothing ? `${ariaDescribedByIds(id)} ${id}-empty` : ariaDescribedByIds(id)}
      aria-invalid={invalid}
      value={typeof value === "string" ? value : ""}
      onChange={(event: ChangeEvent<HTMLSelectElement>) =>
        onChange(event.target.value === "" ? undefined : event.target.value)
      }
      onBlur={(event: FocusEvent<HTMLSelectElement>) => onBlur?.(id, event.target.value)}
      onFocus={(event: FocusEvent<HTMLSelectElement>) => onFocus?.(id, event.target.value)}
    >
      <option value="">{empty}</option>
      {choices.map((choice) => (
        <option key={choice.name} value={choice.name}>
          {choice.label}
        </option>
      ))}
    </Select>
  );

  if (nothing) {
    return (
      <div className="flex flex-col gap-1">
        {select}
        <p id={`${id}-empty`} className="text-caption text-fg-muted">
          {t(emptyText)}
        </p>
      </div>
    );
  }
  if (!failed) {
    return select;
  }
  return (
    <div className="flex flex-col gap-1">
      {select}
      <InlineError id={`${id}-error`}>
        {t("form.listFailed", { reason })}{" "}
        <Button
          variant="ghost"
          size="xs"
          className="px-0 text-caption text-danger underline hover:no-underline"
          onClick={() => {
            void query.refetch();
          }}
        >
          {t("form.listRetry")}
        </Button>
      </InlineError>
    </div>
  );
}

/** The project's manifests of one plural, as select choices titled in the reader's language. */
function useResourceChoices(project: string | undefined, plural: string | undefined, language: string) {
  const query = useQuery({
    queryKey: queryKeys.list(project ?? "", plural ?? ""),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: project as string, plural: plural as string } },
        }),
      ),
    enabled: Boolean(project && plural),
  });
  const choices = useMemo(
    () =>
      asManifests(query.data?.items ?? []).map((manifest) => ({
        name: manifest.metadata.name,
        label: localized(manifest.metadata.title, language, manifest.metadata.name),
      })),
    [query.data, language],
  );
  return { query, choices };
}

/** The empty option's text: loading, nothing to pick (or the read failed), or a prompt to choose. */
function placeholder(t: TFunction, loading: boolean, failed: boolean, count: number): string {
  if (loading) {
    return t("app.loading");
  }
  return failed || count === 0 ? t("form.noResults") : t("form.choose");
}
