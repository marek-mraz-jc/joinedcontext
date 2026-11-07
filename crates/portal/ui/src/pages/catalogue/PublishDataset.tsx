import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { isChange, localized } from "../../api/manifest";
import type { Change, ResourceProposal } from "../../api/manifest";
import { proposeChecked } from "../../api/proposal";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Combobox } from "../../components/pickers/Combobox";
import type { PickerOption } from "../../components/pickers/Combobox";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { Alert, Button, Checkbox, Dialog, Field, Input, Select, Textarea } from "../../components/ui";
import { problemOf } from "./labels";
import { areaLabel, nutsCode, NUTS_CODES } from "./nuts";
import { PublicationPreview } from "./PublicationPreview";
import {
  catalogOf,
  columnsOf,
  formOf,
  FREQUENCIES,
  hiddenOf,
  LICENCES,
  problems,
  textsWith,
  THEMES,
} from "./publish";
import type { CatalogBlock, DraftAttribute, PublishForm } from "./publish";

type Draft = components["schemas"]["CatalogueDraft"];
type Step = "pick" | "describe" | "preview";

interface Drafted {
  draft: Draft;
  manifest: ResourceProposal;
  form: PublishForm;
  /** The dataset's title and description, in the form's language (`metadata.title`, `.description`). */
  title: string;
  description: string;
  /** Offered attributes the person left out of the dataset; they join `projection.hiddenAttributes`. */
  leftOut: string[];
}

function attributesOf(draft: Draft): DraftAttribute[] {
  return (draft.attributes ?? []) as DraftAttribute[];
}

function ckanOf(publish: unknown): Record<string, unknown> {
  const ckan = (publish as { ckan?: unknown } | undefined)?.ckan;
  return ckan && typeof ckan === "object" ? (ckan as Record<string, unknown>) : {};
}

/**
 * Publish a dataset in one step (EP-83, Architecture/21 §7): pick the Endpoint, review the DCAT-AP
 * description the Portal drafted from the Endpoint, its model and the organization, preview the
 * catalogue entry, propose. The proposal is one Change on the Endpoint; nothing is written to the
 * catalogue here, and the dataset appears after the approval, on the reconciler's next run.
 */
export function PublishDatasetDialog({
  project,
  endpoint,
  open,
  onOpenChange,
}: {
  project: string;
  /** Start at the description of this Endpoint rather than at the picker. */
  endpoint?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      size="lg"
      title={t("catalogue.publish.title")}
      description={t("catalogue.publish.lead")}
      closeLabel={t("app.close")}
    >
      {/* Mounted only while open: every opening starts from the draft, never from a half-edit. */}
      {open ? <Flow project={project} endpoint={endpoint} onDone={() => onOpenChange(false)} /> : null}
    </Dialog>
  );
}

function Flow({
  project,
  endpoint,
  onDone,
}: {
  project: string;
  endpoint?: string;
  onDone: () => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const facets = useCatalogueFacets();
  const [picked, setPicked] = useState(endpoint ?? "");
  const [step, setStep] = useState<Step>("pick");
  const [drafted, setDrafted] = useState<Drafted | null>(null);
  const [touched, setTouched] = useState(false);
  const [change, setChange] = useState<Change | null>(null);
  const [confirm, setConfirm] = useState("");

  const load = useMutation({
    mutationFn: async (name: string): Promise<Drafted> => {
      // Neither read waits for the other: one round trip before the form opens, not two.
      const [draft, { apiVersion, kind, metadata, spec }] = await Promise.all([
        api
          .POST("/api/v1/projects/{project}/catalogue/drafts", { params: { path: { project } }, body: { endpoint: name } })
          .then(unwrap),
        api.GET("/api/v1/projects/{project}/{plural}/{name}", { params: { path: { project, plural: "endpoints", name } } }).then(unwrap),
      ]);
      // What the Change carries back is the manifest, never the status the Portal computed.
      const manifest: ResourceProposal = { apiVersion, kind, metadata, spec };
      const language = i18n.resolvedLanguage ?? i18n.language ?? "en";
      const form = formOf(draft.catalog as CatalogBlock, language);
      return {
        draft,
        manifest,
        form,
        title: localized(metadata.title, form.language, metadata.name),
        description: localized(metadata.description, form.language, ""),
        leftOut: [],
      };
    },
    onSuccess: (result) => {
      setDrafted(result);
      setStep("describe");
    },
  });

  const propose = useMutation({
    mutationFn: async (value: Drafted) => {
      const { apiVersion, kind, metadata, spec } = value.manifest;
      const current = spec as Record<string, unknown>;
      const projection = (current.projection ?? {}) as Record<string, unknown>;
      const hidden = hiddenOf(projection.hiddenAttributes, attributesOf(value.draft), value.leftOut);
      const ckan = { ...ckanOf(value.draft.publish) };
      const body = {
        apiVersion,
        kind,
        metadata: {
          ...metadata,
          title: textsWith(metadata.title, value.form.language, value.title),
          description: textsWith(metadata.description, value.form.language, value.description),
        },
        spec: {
          ...current,
          catalog: catalogOf(value.draft.catalog as CatalogBlock, value.form),
          publish: { ...(value.draft.publish as Record<string, unknown>), ckan },
          ...(hidden.length > 0 ? { projection: { ...projection, hiddenAttributes: hidden } } : {}),
          // Publishing a restricted Endpoint would list a dataset nobody browsing can open; the
          // flow said so before this button, and the Change takes the red lane (EP-76, PF-72).
          ...(value.draft.makesPublic ? { audience: "public" } : {}),
        },
      } as ResourceProposal;
      return proposeChecked(project, "endpoints", body, false);
    },
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: ["catalogue"] });
    },
  });

  if (change) {
    return (
      <div className="flex flex-col gap-4">
        <ChangeNotice change={change} project={project} />
        <p className="text-body text-fg-muted">{t("catalogue.publish.afterApproval")}</p>
        <div className="flex justify-end">
          <Button variant="primary" onClick={onDone}>
            {t("catalogue.publish.close")}
          </Button>
        </div>
      </div>
    );
  }

  if (step === "pick" || !drafted) {
    return (
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault();
          if (picked) load.mutate(picked);
        }}
      >
        <ol className="flex gap-3 text-caption text-fg-muted" aria-label={t("catalogue.publish.steps")}>
          <li aria-current="step" className="font-semibold text-fg">
            {t("catalogue.publish.step.pick")}
          </li>
          <li>{t("catalogue.publish.step.describe")}</li>
          <li>{t("catalogue.publish.step.preview")}</li>
        </ol>
        <Field id="publish-endpoint" label={t("catalogue.publish.endpoint")} required description={t("catalogue.publish.endpointHelp")}>
          <ResourceNamePicker
            id="publish-endpoint"
            labelled
            label={t("catalogue.publish.endpoint")}
            from={{ project, plural: "endpoints" }}
            value={picked}
            onChange={setPicked}
            required
          />
        </Field>
        {load.error ? (
          <Alert tone="danger" role="alert">
            {problemOf(load.error, t("app.error.generic"))}
          </Alert>
        ) : null}
        <div className="flex justify-end">
          <Button
            type="submit"
            variant="primary"
            loading={load.isPending}
            disabled={!picked}
            disabledReason={t("catalogue.publish.pickFirst")}
          >
            {t("catalogue.publish.describe")}
          </Button>
        </div>
      </form>
    );
  }

  const found: Partial<Record<keyof PublishForm | "title", string>> = {
    ...problems(drafted.form),
    ...(drafted.title.trim() === "" ? { title: "catalogue.publish.error.title" } : {}),
  };
  const invalid = Object.keys(found).length > 0;
  const errorOf = (field: keyof PublishForm | "title") => (touched && found[field] ? [t(found[field])] : undefined);
  const update = (patch: Partial<PublishForm>) =>
    setDrafted({ ...drafted, form: { ...drafted.form, ...patch } });
  const attributes = attributesOf(drafted.draft);
  const columns = columnsOf(attributes, drafted.leftOut);
  // The area is one region of the table; a value the draft carried that the table lacks stays
  // a choice of its own, so opening the flow never drops it.
  const spatialFirst = drafted.form.spatial.split(",")[0]?.trim() ?? "";
  const areas = spatialFirst && !NUTS_CODES.includes(nutsCode(spatialFirst)) ? [spatialFirst, ...NUTS_CODES] : NUTS_CODES;

  if (step === "preview") {
    return (
      <div className="flex flex-col gap-4">
        <PublicationPreview
          label={t("catalogue.publish.step.preview")}
          title={drafted.title}
          description={drafted.description}
          catalog={catalogOf(drafted.draft.catalog as CatalogBlock, drafted.form)}
          columns={attributes.length > 0 ? columns : undefined}
        />
        {drafted.draft.makesPublic ? (
          <>
            <Alert tone="warning">{t("catalogue.publish.makesPublic")}</Alert>
            {/* The red lane's typed confirmation, asked of the proposer too (CC-39, EP-142). */}
            <Field
              id="publish-confirm"
              label={t("catalogue.publish.confirmLabel")}
              description={t("catalogue.publish.confirmHelp", { name: drafted.draft.endpoint })}
              required
            >
              <Input
                id="publish-confirm"
                value={confirm}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => setConfirm(e.target.value)}
              />
            </Field>
          </>
        ) : null}
        {propose.error ? (
          <Alert tone="danger" role="alert">
            {problemOf(propose.error, t("app.error.generic"))}
          </Alert>
        ) : null}
        <div className="flex justify-between gap-2">
          <Button onClick={() => setStep("describe")}>{t("catalogue.publish.back")}</Button>
          <Button
            variant="primary"
            loading={propose.isPending}
            disabled={drafted.draft.makesPublic && confirm.trim() !== drafted.draft.endpoint}
            disabledReason={t("catalogue.publish.confirmFirst", { name: drafted.draft.endpoint })}
            onClick={() => propose.mutate(drafted)}
          >
            {drafted.draft.makesPublic ? t("catalogue.publish.proposePublic") : t("catalogue.publish.propose")}
          </Button>
        </div>
      </div>
    );
  }

  return (
    <form
      className="flex flex-col gap-4"
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        setTouched(true);
        if (!invalid) setStep("preview");
      }}
    >
      <p className="text-body text-fg-muted">
        {t("catalogue.publish.drafted", { endpoint: drafted.draft.endpoint })}
      </p>
      {drafted.draft.makesPublic ? <Alert tone="warning">{t("catalogue.publish.makesPublic")}</Alert> : null}
      {drafted.draft.missing.length > 0 ? (
        <Alert tone="info">
          {t("catalogue.publish.missing", {
            fields: drafted.draft.missing.map((m) => t(`catalogue.publish.member.${m}`, { defaultValue: m })).join(", "),
          })}
        </Alert>
      ) : null}
      <Field
        id="publish-title"
        label={t("catalogue.publish.datasetTitle")}
        required
        description={t("catalogue.publish.datasetTitleHelp", { language: drafted.form.language })}
        errors={errorOf("title")}
      >
        <Input id="publish-title" value={drafted.title} maxLength={200} onChange={(e) => setDrafted({ ...drafted, title: e.target.value })} />
      </Field>
      <Field
        id="publish-description"
        label={t("catalogue.publish.datasetDescription")}
        description={t("catalogue.publish.datasetDescriptionHelp", { language: drafted.form.language })}
      >
        <Textarea
          id="publish-description"
          rows={3}
          value={drafted.description}
          maxLength={2000}
          onChange={(e) => setDrafted({ ...drafted, description: e.target.value })}
        />
      </Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field id="publish-publisher" label={t("catalogue.facets.publisher")} description={t("catalogue.publish.inLanguage", { language: drafted.form.language })}>
          <Combobox
            id="publish-publisher"
            labelled
            label={t("catalogue.facets.publisher")}
            value={drafted.form.publisher ? [drafted.form.publisher] : []}
            onChange={(values) => update({ publisher: values[0] ?? "" })}
            options={offered(facets.data?.publisher, t("catalogue.publish.inCatalogue"))}
            loading={facets.isPending}
            empty={t("catalogue.publish.typeNew")}
            create={{ label: (text) => t("picker.createNamed", { name: text }), onCreate: (text) => update({ publisher: text.trim().slice(0, 200) }) }}
          />
        </Field>
        <Field id="publish-licence" label={t("catalogue.facets.licence")} required errors={errorOf("license")}>
          <Select id="publish-licence" value={drafted.form.license} onChange={(e) => update({ license: e.target.value })}>
            <option value="">{t("catalogue.publish.choose")}</option>
            {LICENCES.map((licence) => (
              <option key={licence} value={licence}>
                {t(`catalogue.licence.${licence}`)}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="publish-contact-name" label={t("catalogue.publish.contactName")} description={t("catalogue.publish.contactHelp")} errors={errorOf("contactName")}>
          <Input id="publish-contact-name" value={drafted.form.contactName} maxLength={200} onChange={(e) => update({ contactName: e.target.value })} />
        </Field>
        <Field id="publish-contact-email" label={t("catalogue.publish.contactEmail")} errors={errorOf("contactEmail")}>
          <Input id="publish-contact-email" type="email" value={drafted.form.contactEmail} maxLength={254} onChange={(e) => update({ contactEmail: e.target.value })} />
        </Field>
      </div>
      <fieldset className="flex flex-col gap-2">
        <legend className="mb-1 text-body font-medium text-fg">{t("catalogue.facets.theme")}</legend>
        <div className="grid gap-1.5 sm:grid-cols-2">
          {THEMES.map((theme) => (
            <Checkbox
              key={theme}
              label={t(`catalogue.theme.${theme}`)}
              checked={drafted.form.themes.includes(theme)}
              onChange={() =>
                update({
                  themes: drafted.form.themes.includes(theme)
                    ? drafted.form.themes.filter((x) => x !== theme)
                    : [...drafted.form.themes, theme],
                })
              }
            />
          ))}
        </div>
      </fieldset>
      <Field id="publish-keywords" label={t("catalogue.publish.keywords")} description={t("catalogue.publish.keywordsHelp", { language: drafted.form.language })}>
        <Combobox
          id="publish-keywords"
          labelled
          label={t("catalogue.publish.keywords")}
          multiple
          value={keywordList(drafted.form.keywords)}
          onChange={(values) => update({ keywords: values.join(", ") })}
          options={offered(facets.data?.keyword, t("catalogue.publish.inCatalogue"))}
          loading={facets.isPending}
          empty={t("catalogue.publish.typeNew")}
          create={{
            label: (text) => t("picker.createNamed", { name: text }),
            onCreate: (text) => update({ keywords: [...keywordList(drafted.form.keywords), ...keywordList(text)].join(", ") }),
          }}
        />
      </Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field id="publish-spatial" label={t("catalogue.publish.mapExtent")} description={t("catalogue.publish.spatialHelp")}>
          <Select id="publish-spatial" value={spatialFirst} onChange={(e) => update({ spatial: e.target.value })}>
            <option value="">{t("catalogue.publish.noArea")}</option>
            {areas.map((code) => (
              <option key={code} value={code}>
                {areaLabel(code) === code ? code : `${areaLabel(code)} (${nutsCode(code)})`}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="publish-frequency" label={t("catalogue.dataset.frequency")}>
          <Select id="publish-frequency" value={drafted.form.frequency} onChange={(e) => update({ frequency: e.target.value })}>
            <option value="">{t("catalogue.publish.choose")}</option>
            {FREQUENCIES.map((frequency) => (
              <option key={frequency} value={frequency}>
                {t(`catalogue.frequency.${frequency}`)}
              </option>
            ))}
          </Select>
        </Field>
        <Field id="publish-start" label={t("catalogue.publish.start")}>
          <Input id="publish-start" type="date" value={drafted.form.temporalStart} onChange={(e) => update({ temporalStart: e.target.value })} />
        </Field>
        <Field id="publish-end" label={t("catalogue.publish.end")} errors={errorOf("temporalEnd")}>
          <Input id="publish-end" type="date" value={drafted.form.temporalEnd} onChange={(e) => update({ temporalEnd: e.target.value })} />
        </Field>
      </div>
      <fieldset className="flex flex-col gap-2">
        <legend className="mb-1 text-body font-medium text-fg">{t("catalogue.publish.attributes")}</legend>
        <p className="text-caption text-fg-muted">{t("catalogue.publish.attributesHelp")}</p>
        {attributes.length === 0 ? (
          <p className="text-body text-fg-muted">{t("catalogue.publish.noAttributes")}</p>
        ) : (
          [...new Set(attributes.map((a) => a.type))].map((type) => (
            <div key={type} className="flex flex-col gap-1.5">
              <p className="text-caption font-semibold text-fg-muted">{t("catalogue.publish.ofType", { type })}</p>
              <div className="grid gap-1.5 sm:grid-cols-2">
                {attributes
                  .filter((a) => a.type === type)
                  .map((a) => (
                    <Checkbox
                      key={`${type}-${a.name}`}
                      label={a.name}
                      checked={!a.withheld && !drafted.leftOut.includes(a.name)}
                      // The reason shows beside the greyed box, and is read with its name.
                      disabled={a.withheld !== undefined}
                      hint={a.withheld ? t(`catalogue.publish.withheld.${a.withheld}`) : undefined}
                      title={a.withheld ? t(`catalogue.publish.withheld.${a.withheld}`) : undefined}
                      onChange={() =>
                        setDrafted({
                          ...drafted,
                          leftOut: drafted.leftOut.includes(a.name)
                            ? drafted.leftOut.filter((x) => x !== a.name)
                            : [...drafted.leftOut, a.name],
                        })
                      }
                    />
                  ))}
              </div>
            </div>
          ))
        )}
      </fieldset>
      <div className="flex justify-between gap-2">
        <Button onClick={() => setStep("pick")}>{t("catalogue.publish.back")}</Button>
        <Button type="submit" variant="primary">
          {t("catalogue.publish.preview")}
        </Button>
      </div>
    </form>
  );
}

/** The publishers (CKAN organizations) and keywords the catalogue already holds, offered beside free entry (T-4948). */
export function useCatalogueFacets() {
  return useQuery({
    queryKey: ["catalogue", "facets"],
    queryFn: async () => (await unwrap(await api.GET("/api/v1/catalogue", { params: { query: {} } }))).facets,
    staleTime: 5 * 60_000,
  });
}

/** The keywords a comma-separated text names, each once. */
function keywordList(text: string): string[] {
  return [...new Set(text.split(",").map((word) => word.trim()).filter(Boolean))];
}

/** A facet's values as picker options, by the name a person reads. */
function offered(values: { label: string; count: number }[] | undefined, group: string): PickerOption[] {
  return [...new Set((values ?? []).map((value) => value.label))].map((label) => ({ value: label, label, group }));
}
