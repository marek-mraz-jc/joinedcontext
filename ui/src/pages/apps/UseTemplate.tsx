import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { asManifests, isChange, localized } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert, Button, Field, Input, Select } from "../../components/ui";

/** The server's refusals of one field, as `errors` names them (`name: …`, `endpointName: …`). */
export function fieldErrors(error: unknown, field: string): string[] {
  const errors = error instanceof ApiError ? (error.problem?.errors ?? []) : [];
  const prefix = `${field}: `;
  return errors
    .filter((entry): entry is string => typeof entry === "string" && entry.startsWith(prefix))
    .map((entry) => entry.slice(prefix.length));
}

/**
 * A template used as it ships (T-5617): the App's name and the Endpoint it reads, nothing else.
 * The Portal lands the template's source in a repository of its own and proposes the App as a
 * Change; no model is asked, so this works while the app builder cannot answer. The Endpoint
 * list is the server's, which holds only what the person reads, and the server checks the
 * choice again.
 */
export function UseTemplate({ project, template }: { project: string; template: { name: string; title: string } }): JSX.Element {
  const { t, i18n } = useTranslation();
  const id = useId();
  const [name, setName] = useState(template.name);
  const [endpointName, setEndpointName] = useState("");
  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "endpoints" } } })),
  });
  const choices = asManifests(endpoints.data?.items ?? []);
  const create = useMutation({
    mutationFn: async (): Promise<Change | null> => {
      const answered: unknown = await unwrap(
        await api.POST("/api/v1/projects/{project}/app-templates/{name}/apps", {
          params: { path: { project, name: template.name } },
          body: { name: name.trim(), endpointName },
        }),
      );
      return isChange(answered) ? answered : null;
    },
  });
  if (create.data) return <ChangeNotice change={create.data} project={project} />;

  const nameErrors = fieldErrors(create.error, "name");
  const endpointErrors = fieldErrors(create.error, "endpointName");
  const other = create.error && nameErrors.length === 0 && endpointErrors.length === 0 ? create.error : null;
  const endpointHelp = endpoints.isPending
    ? t("app.loading")
    : choices.length === 0 && !endpoints.isError
      ? t("apps.templates.noEndpoints")
      : t("apps.templates.endpointHint");
  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(event) => {
        event.preventDefault();
        create.mutate();
      }}
    >
      <p className="text-body text-fg-muted">{t("apps.templates.useLead", { title: template.title })}</p>
      <Field id={`${id}-name`} label={t("apps.templates.name")} help={t("apps.templates.nameHint")} errors={nameErrors} required>
        <Input id={`${id}-name`} value={name} onChange={(event) => setName(event.target.value)} autoComplete="off" />
      </Field>
      <Field
        id={`${id}-endpoint`}
        label={t("apps.generate.endpoint")}
        help={endpointHelp}
        errors={
          endpoints.isError
            ? [t("form.listFailed", { reason: endpoints.error instanceof ApiError ? (endpoints.error.problem?.detail ?? endpoints.error.message) : t("app.error.generic") })]
            : endpointErrors
        }
        required
      >
        <Select id={`${id}-endpoint`} value={endpointName} onChange={(event) => setEndpointName(event.target.value)}>
          <option value="">{t("apps.generate.pickEndpoint")}</option>
          {choices.map((endpoint) => (
            <option key={endpoint.metadata.name} value={endpoint.metadata.name}>
              {localized(endpoint.metadata.title, i18n.language, endpoint.metadata.name)}
            </option>
          ))}
        </Select>
      </Field>
      {other ? (
        <Alert tone="danger">{other instanceof ApiError ? (other.problem?.detail ?? other.message) : t("app.error.generic")}</Alert>
      ) : null}
      <Button
        type="submit"
        className="w-fit"
        loading={create.isPending}
        disabled={name.trim() === "" || endpointName === ""}
        disabledReason={name.trim() === "" || endpointName === "" ? t("apps.templates.chooseFirst") : undefined}
      >
        {t("apps.templates.useSubmit")}
      </Button>
    </form>
  );
}
