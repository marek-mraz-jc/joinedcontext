import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { api, unwrap } from "../../api/client";
import { isSeq, parseDocument } from "yaml";
import { edit, parseModel } from "./linkml";
import type { Artifacts } from "./LinkmlPreviewPanel";
import { CodeBlock, Alert, Button, Checkbox, EmptyState, Field, Input, RadioGroup, Select, Skeleton, safeHref } from "../../components/ui";

/**
 * The primary path into a model: take an official Smart Data Model and adapt it (DM-07).
 *
 * The browser never reaches the catalogue. It asks the Portal, the Portal asks Model Tools, and
 * Model Tools is the only component with an allowlist that lets it fetch from the Smart Data
 * Models organisation at all (DM-10). The index it answers with is a daily cache; when a
 * refresh could not reach the catalogue the older index is shown as stale rather than withheld,
 * because an editor that cannot browse must still be able to work (DM-12).
 */
export interface CatalogueModel {
  id: string;
  name: string;
  description?: string;
  attributes?: string[];
}

export interface CatalogueSubject {
  name: string;
  title?: string;
  models?: CatalogueModel[];
}

export interface Catalogue {
  /** Models the mirror could not convert: listed with why, never offered (T-6353). */
  failed?: { id: string; finding: string }[];
  subjects?: CatalogueSubject[];
  refreshedAt?: string;
  stale?: boolean;
}

/** The models the mirror could not convert, each with why: named, never offered (T-6353). */
function NotConverted({ failed }: { failed: NonNullable<Catalogue["failed"]> }): React.JSX.Element | null {
  const { t } = useTranslation();
  if (failed.length === 0) return null;
  return (
    <details className="text-body">
      <summary className="cursor-pointer text-fg-muted">{t("models.sdm.notConverted", { count: failed.length })}</summary>
      <p className="mt-1 text-fg-muted">{t("models.sdm.notConvertedHint")}</p>
      <ul className="mt-2 flex max-h-48 flex-col gap-2 overflow-auto">
        {failed.map((one) => (
          <li key={one.id} className="break-words">
            <p className="font-medium">{one.id}</p>
            <p className="text-fg-muted">{one.finding}</p>
          </li>
        ))}
      </ul>
    </details>
  );
}

export interface SmartDataModelsImportProps {
  /** Hands the adapted LinkML source to the editor, with the model it came from (DM-57). */
  onImport: (source: string, model: CatalogueModel) => void;
}

/** How many attributes the panel shows before "show the rest": a list, not a scroll box. */
const ATTRIBUTE_PAGE = 50;

/**
 * How many models the catalogue lists before "show the rest". The catalogue holds about a
 * thousand; all of them at once made a page 92 000 px tall on dev, with the preview left at the
 * top of it (T-1851). Searching is the way through a thousand names, the page is for the first look.
 */
const MODEL_PAGE = 60;

/** Whether a model answers the search, by name, description or attribute name. */
export function matches(model: CatalogueModel, needle: string): boolean {
  const query = needle.trim().toLowerCase();
  if (!query) {
    return true;
  }
  return (
    model.name.toLowerCase().includes(query) ||
    (model.description ?? "").toLowerCase().includes(query) ||
    (model.attributes ?? []).some((attribute) => attribute.toLowerCase().includes(query))
  );
}

/**
 * The imported source with the slots the user did not pick marked deprecated (DM-11).
 *
 * Deleting them would make a federation partner's payload invalid against our own model, so an
 * unused upstream slot stays in the schema and says it is not the one to use.
 */
export function deprecateUnused(source: string, keep: string[]): string {
  const model = parseModel(source);
  const kept = new Set(keep);
  return edit(source, (document) => {
    for (const slot of model.slots) {
      if (!kept.has(slot.name)) {
        document.setIn(["slots", slot.name, "deprecated"], true);
      }
    }
  });
}

/** The ranges an attribute added in the picker may take: the plain ones a person names (T-3251). */
export const ADDED_RANGES = ["string", "integer", "float", "boolean", "datetime"] as const;

export interface AddedAttribute {
  name: string;
  range: (typeof ADDED_RANGES)[number];
}

/** An attribute name as Smart Data Models writes them: lower camel case, letters and digits. */
const ATTRIBUTE_NAME = /^[a-z][A-Za-z0-9]{0,62}$/;

/** What every entity has from NGSI-LD and the `Entity` class of `ngsi-ld-core`: never redeclared. */
const CORE_ATTRIBUTES = ["id", "type", "location", "observedAt", "createdAt", "modifiedAt"];

/**
 * Why a new attribute cannot be added, as a key under `models.sdm.addProblem`: `existing` is every
 * slot the imported document declares, its other classes' included, and the names already added.
 */
export function addedProblem(name: string, existing: string[]): "name" | "taken" | undefined {
  if (!ATTRIBUTE_NAME.test(name)) return "name";
  return existing.includes(name) || CORE_ATTRIBUTES.includes(name) ? "taken" : undefined;
}

/** The imported source with the person's own attributes on its class, each a slot of its own. */
export function withAttributes(source: string, className: string, added: AddedAttribute[]): string {
  if (added.length === 0) return source;
  return edit(source, (document) => {
    for (const attribute of added) {
      // A slot of that name is somebody's already: never overwritten, whatever the caller checked.
      if (document.hasIn(["slots", attribute.name]) || CORE_ATTRIBUTES.includes(attribute.name)) continue;
      document.setIn(["slots", attribute.name], document.createNode({ range: attribute.range }));
      const slots = document.getIn(["classes", className, "slots"]);
      const listed = isSeq(slots) ? (slots.toJSON() as unknown[]) : [];
      document.setIn(["classes", className, "slots"], document.createNode([...listed, attribute.name]));
    }
  });
}

/**
 * On a phone the preview sits under the catalogue, below the dialog's fold: a refusal that
 * appears there scrolls into view, or the person sees nothing happen (T-6106). A stable
 * callback ref runs once per mount.
 */
function revealOnMount(node: HTMLDivElement | null): void {
  node?.scrollIntoView?.({ block: "start" });
}

/**
 * The upstream model the import came from, pinned to its commit (DM-08): the repository, path and
 * commit the importer writes as the model's annotations, as an address a person can open.
 */
export function originOf(source: string): { href: string; commit: string } | undefined {
  const document = parseDocument(source);
  const annotation = (key: string): string | undefined => {
    const value = document.getIn(["annotations", key]);
    const text = value && typeof value === "object" && "value" in value ? (value as { value: unknown }).value : value;
    return typeof text === "string" && text !== "" ? text : undefined;
  };
  const repository = annotation("spec.source.repository");
  const path = annotation("spec.source.path");
  const commit = annotation("spec.source.commit");
  if (!repository || !path || !commit || !/^https:\/\/github\.com\//.test(repository)) return undefined;
  return { href: `${repository.replace(/\/$/, "")}/blob/${commit}/${path.replace(/^\//, "")}`, commit };
}

export function SmartDataModelsImport({
  onImport,
}: SmartDataModelsImportProps): JSX.Element {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const [subject, setSubject] = useState("");
  const [selected, setSelected] = useState<CatalogueModel | null>(null);
  const [refreshes, setRefreshes] = useState(0);
  const [keep, setKeep] = useState<string[] | null>(null);
  /** What the person typed to find an attribute among the two hundred a model may have. */
  const [attribute, setAttribute] = useState("");
  const [showAll, setShowAll] = useState(false);
  const [showAllModels, setShowAllModels] = useState(false);
  // Adopt the model as it is, or adapt it: drop attributes, add the person's own (T-3251).
  const [mode, setMode] = useState<"asIs" | "adapt">("asIs");
  const [added, setAdded] = useState<AddedAttribute[]>([]);
  const [newName, setNewName] = useState("");
  const [newRange, setNewRange] = useState<AddedAttribute["range"]>("string");
  const [addProblem, setAddProblem] = useState<"name" | "taken" | null>(null);

  const catalogue = useQuery({
    queryKey: ["tools", "sdm-catalog", refreshes],
    retry: false,
    queryFn: async (): Promise<Catalogue> =>
      unwrap(
        await api.GET("/api/v1/tools/sdm-catalog", {
          params: { query: refreshes > 0 ? { refresh: true } : {} },
        }),
      ) as Catalogue,
  });

  const preview = useQuery({
    queryKey: ["tools", "import-sdm", selected?.id],
    enabled: Boolean(selected),
    retry: false,
    staleTime: Infinity,
    queryFn: async (): Promise<Artifacts> =>
      unwrap(
        await api.POST("/api/v1/tools/import-sdm", { body: { model: selected?.id ?? "" } }),
      ) as Artifacts,
  });

  const subjects = useMemo(() => catalogue.data?.subjects ?? [], [catalogue.data]);
  const visible = useMemo(
    () =>
      subjects
        .filter((entry) => !subject || entry.name === subject)
        .map((entry) => ({
          ...entry,
          models: (entry.models ?? []).filter((model) => matches(model, search)),
        }))
        .filter((entry) => entry.models.length > 0),
    [subjects, subject, search],
  );
  const matching = visible.reduce((count, entry) => count + entry.models.length, 0);
  const listed = useMemo(() => {
    if (showAllModels) {
      return visible;
    }
    // Each subject takes what the page still has room for after the subjects before it.
    return visible
      .map((entry, index) => {
        const before = visible.slice(0, index).reduce((count, prior) => count + prior.models.length, 0);
        return { ...entry, models: entry.models.slice(0, Math.max(0, MODEL_PAGE - before)) };
      })
      .filter((entry) => entry.models.length > 0);
  }, [visible, showAllModels]);
  const unlisted = matching - listed.reduce((count, entry) => count + entry.models.length, 0);

  const imported = preview.data?.linkml;
  const model = useMemo(() => (imported ? parseModel(imported) : undefined), [imported]);
  const upstreamSlots = useMemo(() => (model?.slots ?? []).map((slot) => slot.name), [model]);
  const chosen = keep ?? upstreamSlots;

  // A model of the catalogue can carry two hundred attributes, which is a scroll box nobody
  // reads (T-1107). What is required comes first, what the person typed narrows it, and the
  // rest waits behind one button rather than filling the panel.
  const offered = useMemo(() => {
    const needle = attribute.trim().toLowerCase();
    const matching = (model?.slots ?? []).filter((slot) =>
      needle === "" ? true : slot.name.toLowerCase().includes(needle),
    );
    const required = matching.filter((slot) => slot.required === true);
    const optional = matching.filter((slot) => slot.required !== true);
    return { required, optional, total: matching.length };
  }, [model, attribute]);
  const shown = showAll
    ? [...offered.required, ...offered.optional]
    : [...offered.required, ...offered.optional].slice(0, ATTRIBUTE_PAGE);
  const hidden = offered.total - shown.length;

  const origin = useMemo(() => (imported ? originOf(imported) : undefined), [imported]);
  const addAttribute = () => {
    const name = newName.trim();
    const problem = addedProblem(name, [...upstreamSlots, ...added.map((one) => one.name)]);
    setAddProblem(problem ?? null);
    if (problem) return;
    setAdded([...added, { name, range: newRange }]);
    setNewName("");
  };

  const doImport = () => {
    if (!imported || !selected) {
      return;
    }
    const className =
      model?.classes.find((klass) => klass.name === selected.name)?.name ?? model?.classes[0]?.name ?? selected.name;
    const adopted =
      mode === "asIs"
        ? imported
        : withAttributes(deprecateUnused(imported, chosen), className, added);
    onImport(adopted, selected);
  };

  return (
    <div className="grid gap-6 lg:grid-cols-2">
      {/* `min-w-0` on both columns: a grid item is as wide as its widest content, and the source's
          long lines made the page scroll sideways at 375 px (T-3251). */}
      <section aria-labelledby="sdm-browse" className="flex min-w-0 flex-col gap-3">
        <h2 id="sdm-browse" className="text-base font-semibold">
          {t("models.sdm.browse")}
        </h2>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-4">
          <Input
            className="w-full"
            type="search"
            aria-label={t("models.sdm.search")}
            placeholder={t("models.sdm.search")}
            value={search}
            onChange={(event) => {
              setSearch(event.target.value);
              setShowAllModels(false);
            }}
          />
          <Select
            aria-label={t("models.sdm.subject")}
            value={subject}
            onChange={(event) => {
              setSubject(event.target.value);
              setShowAllModels(false);
            }}
          >
            <option value="">{t("models.sdm.allSubjects")}</option>
            {subjects.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.title ?? entry.name}
              </option>
            ))}
          </Select>
          <Button
            size="sm"
            onClick={() => setRefreshes((count) => count + 1)}
          >
            {t("models.sdm.refresh")}
          </Button>
        </div>

        {catalogue.isError ? (
          <Alert tone="danger" role="status">
            {t("models.sdm.unavailable")}
          </Alert>
        ) : null}
        {catalogue.data?.stale ? (
          <Alert tone="warning" role="status">
            {t("models.sdm.stale", { at: catalogue.data.refreshedAt ?? "—" })}
          </Alert>
        ) : null}

        {catalogue.isLoading ? (
          <div role="status" className="flex flex-col gap-2">
            <span className="sr-only">{t("models.sdm.catalogueLoading")}</span>
            <Skeleton className="h-4 w-32" />
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        ) : null}
        {matching > 0 ? (
          <p className="text-caption text-fg-muted">{t("models.sdm.count", { count: matching })}</p>
        ) : null}

        <ul
          className={clsx(
            "flex max-h-96 flex-col gap-3 overflow-auto",
            listed.length > 0 && "rounded-md border border-border p-2",
          )}
        >
          {listed.map((entry) => (
            <li key={entry.name}>
              <h3 className="px-2.5 text-caption font-semibold text-fg-muted">
                {entry.title ?? entry.name}
              </h3>
              <ul className="mt-1 flex flex-col gap-1">
                {entry.models.map((model) => (
                  <li key={model.id}>
                    {/* A two-line option of a list, not an action: the shared Button is one
                        line of fixed height, so this stays a plain button with the focus ring. */}
                    <button
                      type="button"
                      onClick={() => {
                        setSelected(model);
                        setKeep(null);
                        setAdded([]);
                        setMode("asIs");
                      }}
                      aria-current={selected?.id === model.id ? "true" : undefined}
                      className={clsx(
                        "focus-ring w-full rounded-md px-2.5 py-1.5 text-left text-body transition-colors",
                        selected?.id === model.id
                          ? "bg-surface-muted font-medium text-fg"
                          : "hover:bg-surface-subtle text-fg",
                      )}
                    >
                      <span className="block font-medium">{model.name}</span>
                      {model.description ? (
                        <span className="block text-caption text-fg-muted">
                          {model.description}
                        </span>
                      ) : null}
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
        {unlisted > 0 ? (
          <Button size="sm" className="self-start" onClick={() => setShowAllModels(true)}>
            {t("models.sdm.showMore", { count: unlisted })}
          </Button>
        ) : null}
        <NotConverted failed={catalogue.data?.failed ?? []} />
        {!catalogue.isLoading && !catalogue.isError && visible.length === 0 ? (
          subjects.length === 0 ? (
            <EmptyState
              className="py-6"
              icon="models"
              title={t("models.sdm.emptyCatalogue")}
              description={t("models.sdm.emptyCatalogueHint")}
            />
          ) : (
            <EmptyState
              className="py-6"
              icon="search"
              title={t("models.sdm.noMatches")}
              description={t("models.sdm.noMatchesHint")}
              action={
                <Button
                  size="sm"
                  onClick={() => {
                    setSearch("");
                    setSubject("");
                  }}
                >
                  {t("models.sdm.clearSearch")}
                </Button>
              }
            />
          )
        ) : null}
      </section>

      <section
        aria-labelledby="sdm-preview"
        className="flex min-w-0 flex-col gap-3 lg:sticky lg:top-4 lg:self-start"
      >
        <h2 id="sdm-preview" className="text-base font-semibold">
          {t("models.sdm.preview")}
        </h2>
        {!selected ? (
          <p className="text-body text-fg-muted">{t("models.sdm.pick")}</p>
        ) : preview.isError ? (
          // Model Tools timing out is the ordinary failure here, and selecting the model again
          // to retry is a step nobody should have to know about (T-1109).
          <Alert
            ref={revealOnMount}
            tone="danger"
            role="status"
            actions={
              <Button size="sm" onClick={() => void preview.refetch()}>
                {t("app.error.retry")}
              </Button>
            }
          >
            {t("models.sdm.previewFailed")}
          </Alert>
        ) : !imported && preview.data?.errors && preview.data.errors.length > 0 ? (
          // Model Tools answers a model it cannot convert with 200 and `errors` (T-6101): the
          // person has to read why, not watch a loading line that never ends.
          <Alert
            ref={revealOnMount}
            tone="danger"
            role="alert"
            actions={
              <Button size="sm" onClick={() => void preview.refetch()}>
                {t("app.error.retry")}
              </Button>
            }
          >
            <p>{t("models.sdm.importRefused", { name: selected.name })}</p>
            <ul className="mt-1 list-disc pl-5">
              {preview.data.errors.map((message, index) => (
                <li key={index}>{message}</li>
              ))}
            </ul>
          </Alert>
        ) : imported ? (
          <>
            {origin && safeHref(origin.href) ? (
              <p className="text-caption text-fg-muted">
                {/* The address comes from the imported model, so it is linked only when it is one (UI-41). */}
                <a href={safeHref(origin.href)} target="_blank" rel="noreferrer noopener" className="underline">
                  {t("models.sdm.origin", { commit: origin.commit.slice(0, 7) })}
                </a>
              </p>
            ) : null}
            <RadioGroup
              name="sdm-mode"
              legend={t("models.sdm.mode")}
              value={mode}
              onChange={setMode}
              options={[
                { value: "asIs", label: t("models.sdm.asIs"), description: t("models.sdm.asIsHint", { count: upstreamSlots.length }) },
                { value: "adapt", label: t("models.sdm.adapt"), description: t("models.sdm.adaptHint") },
              ]}
            />
            {mode === "adapt" ? (
            <>
            <p className="text-body">{t("models.sdm.keepAll")}</p>
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" onClick={() => setKeep(upstreamSlots)}>
                {t("models.sdm.allAttributes")}
              </Button>
              <Button
                size="sm"
                onClick={() =>
                  setKeep(
                    (model?.slots ?? [])
                      .filter((slot) => slot.required === true)
                      .map((slot) => slot.name),
                  )
                }
              >
                {t("models.sdm.onlyRequired")}
              </Button>
              <span role="status" className="text-caption text-fg-muted">
                {t("models.sdm.kept", { kept: chosen.length, total: upstreamSlots.length })}
              </span>
            </div>
            <Input
              className="w-full"
              type="search"
              aria-label={t("models.sdm.findAttribute")}
              placeholder={t("models.sdm.findAttribute")}
              value={attribute}
              onChange={(event) => {
                setAttribute(event.target.value);
                setShowAll(false);
              }}
            />
            <ul className="flex max-h-56 flex-col gap-1 overflow-auto rounded-md border border-border p-2 text-body">
              {shown.map((slot) => (
                <li key={slot.name}>
                  <Checkbox
                    label={slot.name}
                    hint={slot.required ? t("models.sdm.requiredAttribute") : undefined}
                    checked={chosen.includes(slot.name)}
                    onChange={(event) =>
                      setKeep(
                        event.target.checked
                          ? [...chosen, slot.name]
                          : chosen.filter((name) => name !== slot.name),
                      )
                    }
                  />
                </li>
              ))}
            </ul>
            {offered.total === 0 ? (
              <p className="text-caption text-fg-muted">{t("models.sdm.noAttribute")}</p>
            ) : null}
            {hidden > 0 ? (
              <Button size="sm" className="self-start" onClick={() => setShowAll(true)}>
                {t("models.sdm.showMore", { count: hidden })}
              </Button>
            ) : null}
            <fieldset className="flex flex-col gap-2 rounded-md border border-border p-2">
              <legend className="px-1 text-body font-medium">{t("models.sdm.addTitle")}</legend>
              {added.length > 0 ? (
                <ul className="flex flex-wrap gap-1" aria-label={t("models.sdm.addedList")}>
                  {added.map((one) => (
                    <li key={one.name} className="flex items-center gap-1 rounded border border-border px-2 py-0.5 font-mono text-caption">
                      {one.name}: {one.range}
                      <Button
                        size="sm"
                        variant="ghost"
                        aria-label={t("models.sdm.addRemove", { name: one.name })}
                        onClick={() => setAdded(added.filter((other) => other.name !== one.name))}
                      >
                        ×
                      </Button>
                    </li>
                  ))}
                </ul>
              ) : null}
              <div className="flex flex-wrap items-end gap-2">
                <Field
                  id="sdm-add-name"
                  label={t("models.sdm.addName")}
                  help={t("models.sdm.addNameHelp")}
                  errors={addProblem ? [t(`models.sdm.addProblem.${addProblem}`)] : undefined}
                >
                  <Input
                    id="sdm-add-name"
                    value={newName}
                    onChange={(event) => setNewName(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        addAttribute();
                      }
                    }}
                  />
                </Field>
                <Field id="sdm-add-range" label={t("models.sdm.addRange")}>
                  <Select
                    id="sdm-add-range"
                    value={newRange}
                    onChange={(event) => setNewRange(event.target.value as AddedAttribute["range"])}
                  >
                    {ADDED_RANGES.map((range) => (
                      <option key={range} value={range}>
                        {t(`models.sdm.range.${range}`)}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Button size="sm" onClick={addAttribute}>
                  {t("models.sdm.add")}
                </Button>
              </div>
            </fieldset>
            </>
            ) : null}
            <CodeBlock label={t("codeBlock.code")} className="max-h-56 rounded border border-border bg-surface-subtle p-3 font-mono text-xs text-fg">
              {imported}
            </CodeBlock>
            <Button
              variant="primary"
              size="sm"
              className="self-start"
              onClick={doImport}
            >
              {t("models.sdm.import", { name: selected.name })}
            </Button>
          </>
        ) : (
          <p className="text-body text-fg-muted">{t("models.sdm.loading")}</p>
        )}
      </section>
    </div>
  );
}
