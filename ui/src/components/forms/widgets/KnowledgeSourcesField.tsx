import { useContext } from "react";
import type { JSX } from "react";
import type { FieldProps } from "@rjsf/utils";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../../api/client";
import { projectsQuery } from "../../../api/projects";
import { asManifests, localized } from "../../../api/manifest";
import { Badge, Checkbox } from "../../ui";
import { useShownErrors } from "../touched";
import { FormDataContext } from "./EntitySelectorField";
import { LIST_BODY, LIST_FIELDSET, ListLegend } from "../theme";
import { ListFailed } from "./ListFailed";
import { FormProjectContext } from "./ModelWidgets";

/** A `KnowledgeSource` as the picker offers it: its reference in the deployment, and whether a public channel may read it. */
interface Offered {
  ref: string;
  label: string;
  name: string;
  public: boolean;
}

/**
 * A deployment's `sources`, picked from every project the person reads (T-4281 item 8): the own
 * project's by name, another's as `{project}/{source}`, which puts that project in `scope.projects`
 * when the form is proposed. A channel nobody signs in to reads public sources alone (MF-52), so an
 * internal source is offered there only to be taken away. The lists are the server's answer to this
 * person, so nothing is offered they could not open.
 */
export function KnowledgeSourcesField(props: FieldProps): JSX.Element {
  const { formData, onChange, disabled, readonly, schema, rawErrors, fieldPathId } = props;
  // Its help sits under its legend, above the sources, as every field's does (T-4311).
  const help = typeof props.uiSchema?.["ui:help"] === "string" ? props.uiSchema["ui:help"] : undefined;
  const id = fieldPathId.$id;
  const { t, i18n } = useTranslation();
  const errors = useShownErrors(id, rawErrors);
  const own = useContext(FormProjectContext);
  const root = useContext(FormDataContext) as { channel?: unknown } | undefined;
  const anonymous = typeof root?.channel === "string" && root.channel !== "internal";
  const chosen: string[] = Array.isArray(formData) ? (formData as unknown[]).filter((v): v is string => typeof v === "string") : [];

  const projects = useQuery({
    ...projectsQuery,
    select: (list) => list.items.map((item) => item.name),
  });
  // The own project first, then the others as the server lists them.
  const names = [...new Set([...(own ? [own] : []), ...(projects.data ?? [])])];
  const lists = useQueries({
    queries: names.map((project) => ({
      queryKey: queryKeys.list(project, "knowledgesources"),
      queryFn: async () =>
        unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "knowledgesources" } } })),
    })),
  });

  const byProject = names.map((project, at) => {
    const offered: Offered[] = asManifests(lists[at]?.data?.items ?? []).map((manifest) => {
      const visibility = (manifest.spec as { visibility?: unknown } | undefined)?.visibility;
      return {
        ref: project === own ? manifest.metadata.name : `${project}/${manifest.metadata.name}`,
        label: localized(manifest.metadata.title, i18n.language, manifest.metadata.name),
        name: manifest.metadata.name,
        public: visibility === "public",
      };
    });
    return { project, offered, query: lists[at] };
  });
  // What the manifest names that no list offers (a hand-written or since-removed source) stays visible, to be taken away.
  const known = new Set(byProject.flatMap((group) => group.offered.map((o) => o.ref)));
  const unknown = chosen.filter((ref) => !known.has(ref));
  const frozen = Boolean(disabled || readonly);
  const toggle = (ref: string, on: boolean): void => {
    const next = on ? [...chosen, ref] : chosen.filter((held) => held !== ref);
    onChange(next.length > 0 ? next : undefined, fieldPathId.path);
  };

  const box = (ref: string, label: JSX.Element, refused: boolean) => (
    <Checkbox
      key={ref}
      id={`${id}-${ref.replace("/", "--")}`}
      checked={chosen.includes(ref)}
      // On a public channel the hint says why an internal source cannot be ticked.
      aria-describedby={anonymous ? `${id}-hint` : undefined}
      // An internal source on a public channel can be taken away, never added.
      disabled={frozen || (refused && !chosen.includes(ref))}
      onChange={(event) => toggle(ref, event.currentTarget.checked)}
      // A name and its badge wrap to two lines at a phone's width: the box stays on the name's (T-4311).
      className="items-start [&>input]:mt-1"
      label={label}
    />
  );

  return (
    <fieldset id={id} data-testid="knowledge-sources" className={LIST_FIELDSET}>
      <ListLegend heading={schema.title ?? ""} required={props.required} />
      <div className={LIST_BODY}>
      {help ? <p id={`${id}__help`} className="text-caption text-fg-muted">{help}</p> : null}
      {anonymous ? <p id={`${id}-hint`} className="text-caption text-fg-muted">{t("knowledge.sourcesPicker.publicOnly")}</p> : null}
      {projects.isError ? <ListFailed what={t("nav.projects")} reason={reasonOf(projects.error, t("app.error"))} onRetry={() => void projects.refetch()} /> : null}
      {byProject.map(({ project, offered, query }) =>
        query?.isError || offered.length > 0 ? (
          <div key={project} role="group" aria-label={t("knowledge.sourcesPicker.of", { project })} className="flex flex-col gap-1">
            <p className="text-caption font-semibold text-fg-muted">
              {project === own ? t("knowledge.sourcesPicker.own", { project }) : project}
            </p>
            {query?.isError ? (
              <ListFailed what={schema.title ?? ""} reason={reasonOf(query.error, t("app.error"))} onRetry={() => void query.refetch()} />
            ) : null}
            {offered.map((o) =>
              box(
                o.ref,
                <span className="flex flex-wrap items-center gap-2">
                  <span title={o.ref}>{o.label}</span>
                  {!o.public ? <Badge tone={anonymous ? "warning" : "neutral"}>{t("knowledge.visibility.internal")}</Badge> : null}
                </span>,
                anonymous && !o.public,
              ),
            )}
          </div>
        ) : null,
      )}
      {unknown.length > 0 ? (
        <div role="group" aria-label={t("knowledge.sourcesPicker.unknown")} className="flex flex-col gap-1">
          <p className="text-caption font-semibold text-fg-muted">{t("knowledge.sourcesPicker.unknown")}</p>
          {unknown.map((ref) => box(ref, <span className="font-mono">{ref}</span>, true))}
        </div>
      ) : null}
      {!projects.isLoading && byProject.every((group) => group.offered.length === 0 && !group.query?.isError) && unknown.length === 0 ? (
        <p className="text-caption text-fg-muted">{t("knowledge.sourcesPicker.none")}</p>
      ) : null}
      {errors && errors.length > 0 ? <p className="text-caption text-danger">{errors[0]}</p> : null}
      </div>
    </fieldset>
  );
}
