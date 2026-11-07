/**
 * One data model's own page, `/projects/{project}/models/{name}` (DM-61, DM-62, T-2765).
 *
 * What the model is (its classes, their fields, what relates to what, its enums), the form each
 * class generates, the LinkML, who uses it, what changed it, and its Mappings. Read-only: Edit
 * opens the editor for a person who may propose a DataModel and says why not to anyone else.
 */
import { RecordLink } from "../../components/RecordLink";
import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { readModelSource } from "../../api/datamodelSource";
import { localized } from "../../api/manifest";
import type { Change, Manifest } from "../../api/manifest";
import { ResourceHistory } from "../../components/ResourceHistory";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import {
  Alert,
  Badge,
  Button,
  buttonClass,
  EmptyState,
  Icon,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  Tabs,
  tabPanelProps,
} from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { LinkmlGraphView } from "./LinkmlGraphView";
import { MappingsEditor } from "./MappingsEditor";
import type { MappingModel } from "./MappingsEditor";
import { ModelFields } from "./ModelFields";
import { ModelForm, ModelYaml, useImportSources } from "./ModelViews";
import { ProposeLink, useProjectList, useProjectManifests } from "./ModelsList";
import { classSlots, isAnyClass, parseModel, valueClasses } from "./linkml";
import { fieldWord, useFieldWords } from "../../components/entities/fieldTypes";
import { AddFieldDialog } from "../../components/entities/AddFieldDialog";
import { usePermissions } from "../../api/permissions";
import { useOrgDomain } from "../../api/projects";
import { spaceOf } from "../../components/endpoints/sharing";
import type { LinkmlModel } from "./linkml";
import { OrganizationCopyOffer, ShareWithOrganization } from "./ShareModel";
import { spaceOfModel, USING_KINDS, usesOfModel } from "./modelUsage";
import type { ModelUse } from "./modelUsage";
import { useUrlParam } from "../../navigation/urlState";

type View = "overview" | "fields" | "form" | "yaml" | "used" | "history" | "mappings";
const VIEWS: View[] = ["overview", "fields", "form", "yaml", "used", "history", "mappings"];

/** The source of a model: the manifest's own text, else the file the repository keeps (DM-56). */
export function useSourceOf(project: string, model: Manifest | undefined) {
  const inline = typeof model?.spec.linkml === "string" && model.spec.linkml.includes("\n") ? model.spec.linkml : undefined;
  return useQuery({
    queryKey: ["datamodel-source", project, model?.metadata.name],
    enabled: model !== undefined && inline === undefined,
    retry: false,
    queryFn: () => readModelSource(project, model?.metadata.name ?? ""),
    // The inline text is the answer as it stands; nothing is fetched for it.
    initialData: inline,
  });
}

/** A link to one user of the model: its own page where it has one, else its edit form. */
export function UseLink({ project, use }: { project: string; use: Pick<ModelUse, "plural" | "name"> }): JSX.Element {
  return <RecordLink project={project} plural={use.plural} name={use.name} className="font-mono" />;
}

/**
 * The model's types as tables of fields in plain words (T-4829): "Number", "Link to Building",
 * a value object's fields inside the field that holds it. Add field is the space's own dialog
 * (B11), which proposes a DataModel Change and never writes. Who may not propose sees no button:
 * a read-only page, not a disabled form (V03).
 */
function ClassCards({
  project,
  name,
  source,
  model,
  space,
  endpoints,
}: {
  project: string;
  name: string;
  source: string;
  model: LinkmlModel;
  space?: string;
  endpoints: Manifest[];
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const word = useFieldWords();
  const { can } = usePermissions(project);
  const orgDomain = useOrgDomain(project);
  const [adding, setAdding] = useState<string | null>(null);
  const [proposed, setProposed] = useState<Change[]>([]);
  const hidden = valueClasses(model);
  const types = model.classes.filter((klass) => !isAnyClass(klass) && !hidden.has(klass.name));
  const mayPropose = can("DataModel", "propose");
  return (
    <section aria-labelledby="model-types" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="model-types" className="text-sm font-semibold">
          {t("models.classes")}
        </h2>
        {mayPropose ? (
          <ProposeLink project={project} search={{ edit: name }}>
            {t("models.page.addType")}
          </ProposeLink>
        ) : null}
      </div>
      {proposed.map((change) => (
        <ChangeNotice key={change.metadata.name} change={change} project={project} />
      ))}
      <ul className="grid grid-cols-1 gap-3 md:grid-cols-2">
        {types.map((klass) => {
          const fields = classSlots(model, klass);
          const title = localized(klass.title, i18n.language, klass.name);
          return (
            <li key={klass.name} className="flex min-w-0 flex-col gap-2 rounded border border-border p-3">
              <div className="flex flex-wrap items-start justify-between gap-2">
                <h3 className="min-w-0 text-body font-semibold [overflow-wrap:anywhere]">
                  {title}
                  {title !== klass.name ? <span className="ml-2 font-mono text-caption text-fg-muted">{klass.name}</span> : null}
                </h3>
                {mayPropose ? (
                  <Button size="sm" variant="secondary" aria-label={t("models.page.addFieldTo", { type: title })} onClick={() => setAdding(klass.name)}>
                    {t("models.page.addField")}
                  </Button>
                ) : null}
              </div>
              {klass.description ? <p className="text-caption text-fg-muted">{klass.description}</p> : null}
              {fields.length === 0 ? (
                <p className="text-body text-fg-muted">{t("models.page.noFields")}</p>
              ) : (
                <Table caption={t("models.page.typeCaption", { type: title })}>
                  <TableHead>
                    <TableHeaderCell>{t("models.slot")}</TableHeaderCell>
                    <TableHeaderCell>{t("models.range")}</TableHeaderCell>
                  </TableHead>
                  <TableBody>
                    {fields.map((slot) => (
                      <TableRow key={slot.name}>
                        <TableCell primary>{localized(slot.title, i18n.language, slot.name)}</TableCell>
                        <TableCell>
                          {word(fieldWord(slot, model))}
                          {slot.required ? <span className="text-fg-muted"> · {t("models.required")}</span> : null}
                          {slot.multivalued ? <span className="text-fg-muted"> · {t("models.multivalued")}</span> : null}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </li>
          );
        })}
      </ul>
      {mayPropose && adding !== null ? (
        <AddFieldDialog
          project={project}
          modelName={name}
          source={source}
          type={adding}
          open
          onOpenChange={(open) => {
            if (!open) setAdding(null);
          }}
          onProposed={(change) => setProposed((before) => [...before, change])}
          space={space}
          endpoints={endpoints.filter((endpoint) => spaceOf(endpoint) === space)}
          orgDomain={orgDomain}
        />
      ) : null}
    </section>
  );
}

/** The model's Mappings to and from the project's other models (DM-33), proposed from here. */
function ModelMappings({
  project,
  model,
  source,
  models,
  spaces,
}: {
  project: string;
  model: Manifest;
  source: string;
  models: Manifest[];
  spaces: Manifest[];
}): JSX.Element {
  const [proposed, setProposed] = useState<Change | null>(null);
  const others = models.filter((other) => other.metadata.name !== model.metadata.name);
  const sources = useQueries({
    queries: others.map((other) => ({
      queryKey: ["datamodel-source", project, other.metadata.name],
      retry: false,
      queryFn: () => readModelSource(project, other.metadata.name),
    })),
  });
  const version = (manifest: Manifest) => (typeof manifest.spec.version === "string" ? manifest.spec.version : "1.0.0");
  const mappable: MappingModel[] = [
    { name: model.metadata.name, version: version(model), source },
    ...others.flatMap((other, index) => {
      const text = sources[index]?.data;
      return text === undefined ? [] : [{ name: other.metadata.name, version: version(other), source: text }];
    }),
  ];
  const byName = new Map(models.map((one) => [one.metadata.name, one]));
  return (
    <div className="flex flex-col gap-3">
      {proposed ? <ChangeNotice change={proposed} project={project} /> : null}
      <MappingsEditor
        project={project}
        models={mappable}
        spaceOf={(name) => {
          const one = byName.get(name);
          return one ? spaceOfModel(one, spaces) : undefined;
        }}
        onProposed={setProposed}
      />
    </div>
  );
}

export function ModelPage({
  project,
  name,
  initialClass,
}: {
  project: string;
  name: string;
  /** A class to open the form on, when a link named a type of the model. */
  initialClass?: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  // The tab is in the address, so back, reload and a sent link open it (T-3239).
  const [view, setView] = useUrlParam<View>("tab", initialClass ? "form" : "overview", VIEWS);
  const models = useProjectList(project, "datamodels");
  const all = useProjectManifests(project);
  const model = models.data?.find((one) => one.metadata.name === name);
  const source = useSourceOf(project, model);
  const parsed = useMemo(() => (source.data ? parseModel(source.data) : undefined), [source.data]);
  const uses = useMemo(() => (model ? usesOfModel(model, all) : []), [model, all]);
  const imports = useImportSources(project, name, source.data ?? "");

  const back = (
    <Link
      to="/projects/$project/models"
      params={{ project }}
      className="focus-ring text-body text-primary-soft-fg underline hover:no-underline"
    >
      {t("models.page.back")}
    </Link>
  );

  if (models.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (models.isError) {
    return (
      <ResourcePageFailed
        title={name}
        description={t("models.page.listLead")}
        error={models.error}
        onRetry={() => void models.refetch()}
        back={back}
      />
    );
  }
  if (model === undefined) {
    return (
      <div className="flex flex-col gap-4">
        <PageHeader title={name} description={t("models.page.listLead")} />
        <EmptyState
          icon="models"
          title={t("models.page.notFound", { name })}
          description={t("models.page.notFoundLead")}
          action={back}
        />
      </div>
    );
  }

  const space = spaceOfModel(model, all.spaces);
  const version = typeof model.spec.version === "string" ? model.spec.version : undefined;
  const lifecycle = typeof model.spec.lifecycle === "string" ? model.spec.lifecycle : undefined;

  return (
    <div className="flex flex-col gap-4">
      {/* One row: the title, what it is in brackets, and the model's actions; the breadcrumb leads back to the list (owner 2026-10-31). */}
      <PageHeader
        compact
        title={localized(model.metadata.title, i18n.language, name)}
        description={localized(model.metadata.description, i18n.language, "") || t("models.page.modelLead")}
        titleAction={
          <span className="ms-2 flex flex-wrap items-center gap-2 text-body text-fg-muted">
            <span className="font-mono">
              ({name}
              {version ? ` v${version}` : ""})
            </span>
            {lifecycle ? <Badge>{t(`models.lifecycleOption.${lifecycle}`, { defaultValue: lifecycle })}</Badge> : null}
          </span>
        }
        actions={
          <>
            <ProposeLink project={project} search={{ edit: name }} variant="primary">
              {t("models.page.edit")}
            </ProposeLink>
            <ShareWithOrganization project={project} name={name} version={version} lifecycle={lifecycle} />
            {/* Removal proposes a Red change (CC-19); shown only to a role that may delete a DataModel. */}
            <DeleteResourceAction
              target={{ project, kind: "DataModel", plural: "datamodels", name, label: localized(model.metadata.title, i18n.language, name) }}
            />
          </>
        }
      />

      {source.data !== undefined ? <OrganizationCopyOffer project={project} name={name} source={source.data} /> : null}

      <Tabs
        id="model-page"
        label={t("models.page.views")}
        tabs={VIEWS.map((one) => ({ value: one, label: t(`models.page.view.${one}`) }))}
        value={view}
        onChange={setView}
      />
      <div {...tabPanelProps("model-page", view)}>
        {source.isError && (view === "overview" || view === "fields" || view === "form" || view === "yaml" || view === "mappings") ? (
          <Alert tone="danger" role="alert">
            <p>{t("models.page.sourceFailed", { reason: source.error instanceof Error ? source.error.message : "" })}</p>
            <Button size="sm" className="mt-2" onClick={() => void source.refetch()}>
              {t("app.error.retry")}
            </Button>
          </Alert>
        ) : source.data === undefined && view !== "used" && view !== "history" ? (
          <p role="status" className="text-body text-fg-muted">
            {t("models.source.loading")}
          </p>
        ) : (
          <>
            {view === "overview" && source.data !== undefined && parsed ? (
              <div className="flex flex-col gap-4">
                {/* One toolbar row ending in Open full screen; the relationships go below the
                    drawing, so it spans the page (owner, T-6146). */}
                <LinkmlGraphView
                  source={source.data}
                  imports={imports}
                  toolbarEnd={
                    <Link to="/projects/$project/models/$name/diagram" params={{ project, name }} className={buttonClass("secondary", "sm")}>
                      <Icon name="expand" className="size-4" />
                      {t("models.canvas.open")}
                    </Link>
                  }
                  onOpenClass={() => {
                    setView("form");
                  }}
                />
                <ClassCards
                  project={project}
                  name={name}
                  source={source.data}
                  model={parsed}
                  space={space}
                  endpoints={all.endpoints ?? []}
                />
                {parsed.enums.length > 0 ? (
                  <section aria-labelledby="model-enums" className="flex flex-col gap-2">
                    <h2 id="model-enums" className="text-sm font-semibold">
                      {t("models.enums")}
                    </h2>
                    <ul className="flex flex-col gap-1 text-body">
                      {parsed.enums.map((entry) => (
                        <li key={entry.name}>
                          <span className="font-mono font-medium">{entry.name}</span>:{" "}
                          {entry.permissible_values
                            .map((value) => (value.title ? `${value.name} (${localized(value.title, i18n.language, value.name)})` : value.name))
                            .join(", ")}
                        </li>
                      ))}
                    </ul>
                  </section>
                ) : null}
              </div>
            ) : null}
            {view === "fields" && parsed ? <ModelFields project={project} model={parsed} space={space} /> : null}
            {view === "form" && source.data !== undefined ? <ModelForm source={source.data} initialClass={initialClass} /> : null}
            {view === "yaml" && source.data !== undefined ? <ModelYaml source={source.data} name={name} /> : null}
            {view === "used" ? (
              uses.length === 0 ? (
                <p className="text-body text-fg-muted">{t("models.page.unusedLead")}</p>
              ) : (
                <dl className="grid gap-x-6 gap-y-2 sm:grid-cols-[12rem_1fr]">
                  {USING_KINDS.filter(({ kind }) => uses.some((use) => use.kind === kind)).map(({ kind }) => (
                    <div key={kind} className="contents">
                      <dt className="font-medium">{t(`models.page.usedKind.${kind}`)}</dt>
                      <dd>
                        <ul className="flex flex-wrap gap-x-3 gap-y-1">
                          {uses
                            .filter((use) => use.kind === kind)
                            .map((use) => (
                              <li key={use.name}>
                                <UseLink project={project} use={use} />
                              </li>
                            ))}
                        </ul>
                      </dd>
                    </div>
                  ))}
                </dl>
              )
            ) : null}
            {view === "history" ? (
              // The merged and rejected Changes of this model, the published v1.0.0 one included (T-4830).
              <ResourceHistory project={project} kind="DataModel" name={name} />
            ) : null}
            {view === "mappings" && source.data !== undefined ? (
              <ModelMappings
                project={project}
                model={model}
                source={source.data}
                models={models.data ?? []}
                spaces={all.spaces ?? []}
              />
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}
