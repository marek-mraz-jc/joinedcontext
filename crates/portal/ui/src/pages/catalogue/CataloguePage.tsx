import { Suspense, lazy, useState } from "react";
import type { JSX } from "react";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  Dialog,
  EmptyState,
  Icon,
  Input,
  PageFailed,
  PageHeader,
  PageLoading,
  PermissionGuard,
} from "../../components/ui";
import { useThemeLabel } from "./labels";
import { useAuthStatus } from "../../auth/AuthProvider";
import { ReadableDatasets } from "./ReadableDatasets";

export { problemOf, useThemeLabel } from "./labels";

// The publish dialog is for a signed-in person in a project; a visitor never downloads it (T-5122).
const PublishDatasetDialog = lazy(() => import("./PublishDataset").then((module) => ({ default: module.PublishDatasetDialog })));
import { FACETS, narrowed, toggled } from "./search";
import type { CatalogueSearch, FacetName } from "./search";
import { areaLabel } from "./nuts";
import { catalogueQuery } from "./queries";

type CataloguePageAnswer = components["schemas"]["CataloguePage"];
type CatalogueDataset = components["schemas"]["CatalogueDataset"];
type FacetValue = components["schemas"]["CatalogueFacetValue"];


/**
 * Every public dataset of the installation, searched and faceted (EP-81, Architecture/21 §6).
 *
 * The page reads what the catalogues hold, through the Portal's anonymous read of CKAN, so it
 * shows the same to a citizen with a link as to a steward who is signed in; nothing on it needs
 * a session. The search lives in the address, so a filtered list is a link one can share.
 */
export function CataloguePage({
  project,
  search,
  onSearch,
}: {
  /** The project a signed-in person works in; a visitor has none and publishes nothing. */
  project: string | null;
  search: CatalogueSearch;
  onSearch: (next: CatalogueSearch) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [publishing, setPublishing] = useState(false);
  // Signed in: the internal datasets the gateway grants the person come first (T-6411).
  const signedIn = useAuthStatus() === "authenticated";
  const catalogue = useQuery({
    ...catalogueQuery(search),
    placeholderData: keepPreviousData,
  });

  const header = (
    <PageHeader
      title={t("catalogue.title")}
      description={t(signedIn ? "catalogue.leadSignedIn" : "catalogue.lead")}
      actions={
        project ? (
          <PermissionGuard project={project} kind="Endpoint" verb="propose">
            <Button icon={<Icon name="ckan" className="size-4" />} onClick={() => setPublishing(true)}>
              {t("catalogue.publish.open")}
            </Button>
          </PermissionGuard>
        ) : undefined
      }
    />
  );

  return (
    <div className="flex flex-col gap-section">
      {header}
      {signedIn ? <ReadableDatasets /> : null}
      {project ? (
        <Suspense fallback={null}>
          <PublishDatasetDialog project={project} open={publishing} onOpenChange={setPublishing} />
        </Suspense>
      ) : null}
      {/* Keyed by the address's text: a back button or a shared link resets the box to it. */}
      <SearchForm key={search.q ?? ""} search={search} onSearch={onSearch} />

      {catalogue.isPending ? (
        <PageLoading label={t("catalogue.loading")} />
      ) : catalogue.isError ? (
        <PageFailed
          error={catalogue.error}
          onRetry={() => {
            void catalogue.refetch();
          }}
        />
      ) : (
        <Results
          answer={catalogue.data}
          search={search}
          onSearch={onSearch}
          refreshing={catalogue.isFetching}
        />
      )}
    </div>
  );
}

function SearchForm({
  search,
  onSearch,
}: {
  search: CatalogueSearch;
  onSearch: (next: CatalogueSearch) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [text, setText] = useState(search.q ?? "");
  return (
    <form
      role="search"
      aria-label={t("catalogue.search.label")}
      className="flex flex-wrap items-end gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        const q = text.trim();
        onSearch({ ...search, q: q === "" ? undefined : q, page: undefined });
      }}
    >
      <label className="flex min-w-64 flex-1 flex-col gap-1 text-body font-medium text-fg">
        {t("catalogue.search.label")}
        <Input
          type="search"
          value={text}
          maxLength={200}
          placeholder={t("catalogue.search.placeholder")}
          onChange={(event) => setText(event.target.value)}
        />
      </label>
      <Button type="submit" variant="primary" icon={<Icon name="search" className="size-4" />}>
        {t("catalogue.search.submit")}
      </Button>
      {narrowed(search) ? (
        <Button type="button" variant="ghost" onClick={() => onSearch({})}>
          {t("catalogue.search.clear")}
        </Button>
      ) : null}
    </form>
  );
}

function Results({
  answer,
  search,
  onSearch,
  refreshing,
}: {
  answer: CataloguePageAnswer;
  search: CatalogueSearch;
  onSearch: (next: CatalogueSearch) => void;
  refreshing: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [filtersOpen, setFiltersOpen] = useState(false);
  const pages = Math.max(1, Math.ceil(answer.total / answer.pageSize));
  const chosen = FACETS.reduce((sum, facet) => sum + (search[facet]?.length ?? 0), 0);
  // A filter whose every value nearly every listed dataset has (Format: 625 of 626 on prod) narrows
  // nothing: last, closed (T-7317). Publisher and Theme, the filters people pick by, stay open.
  const idle = (facet: FacetName) =>
    facet !== "publisher" &&
    facet !== "theme" &&
    !search[facet]?.length &&
    answer.total > 0 &&
    answer.facets[facet].every((v) => v.count >= answer.total * IDLE_SHARE);
  const groups = [...FACETS.filter((f) => !idle(f)), ...FACETS.filter(idle)].map((facet) => (
    <FacetGroup
      key={facet}
      facet={facet}
      values={answer.facets[facet]}
      selected={search[facet] ?? []}
      idle={idle(facet)}
      onToggle={(value) => onSearch(toggled(search, facet, value))}
    />
  ));
  const clearFilters = () => onSearch(search.q ? { q: search.q } : {});
  return (
    <div className="flex flex-col gap-6 md:flex-row md:items-start">
      {/* Under 768 px the facets wait behind a button, so the first dataset is on the first screen (T-4995). */}
      <aside
        aria-label={t("catalogue.facets.label")}
        className="hidden flex-col gap-4 md:flex md:w-60 md:shrink-0 lg:w-72"
      >
        {groups}
      </aside>
      <Dialog
        open={filtersOpen}
        onOpenChange={setFiltersOpen}
        title={t("catalogue.facets.label")}
        description={t("catalogue.facets.hint")}
        closeLabel={t("catalogue.facets.close")}
        className="h-[88vh]"
        footer={
          <Button variant="primary" onClick={() => setFiltersOpen(false)}>
            {t("catalogue.facets.show", { count: answer.total })}
          </Button>
        }
      >
        <div className="flex flex-col gap-4">
          <Button
            variant="secondary"
            size="sm"
            className="self-end"
            disabled={chosen === 0}
            disabledReason={t("catalogue.facets.noneChosen")}
            onClick={clearFilters}
          >
            {t("catalogue.facets.clear")}
          </Button>
          {groups}
        </div>
      </Dialog>
      <section aria-label={t("catalogue.results.label")} aria-busy={refreshing} className="flex min-w-0 flex-1 flex-col gap-4">
        {answer.unavailable.length > 0 ? (
          <Alert tone="warning">
            {t("catalogue.unavailable", { catalogues: answer.unavailable.join(", ") })}
          </Alert>
        ) : null}
        <div className="flex items-center justify-between gap-2">
          <p role="status" className="text-body text-fg-muted">
            {t("catalogue.results.count", { count: answer.total })}
          </p>
          <Button
            variant="secondary"
            className="md:hidden"
            aria-haspopup="dialog"
            onClick={() => setFiltersOpen(true)}
          >
            {t("catalogue.facets.open", { count: chosen })}
          </Button>
        </div>
        {chosen > 0 ? <ChosenFilters search={search} answer={answer} onSearch={onSearch} onClear={clearFilters} /> : null}
        {answer.datasets.length === 0 ? (
          <EmptyState
            icon="ckan"
            title={narrowed(search) ? t("catalogue.results.noMatch") : t("catalogue.results.empty")}
            description={
              narrowed(search) ? t("catalogue.results.noMatchHint") : t("catalogue.results.emptyHint")
            }
            action={
              narrowed(search) ? (
                <Button variant="secondary" onClick={() => onSearch({})}>
                  {t("catalogue.search.clear")}
                </Button>
              ) : undefined
            }
          />
        ) : (
          <ul className="flex flex-col gap-3">
            {answer.datasets.map((dataset) => (
              <li key={dataset.name}>
                <DatasetCard dataset={dataset} />
              </li>
            ))}
          </ul>
        )}
        {pages > 1 ? (
          <nav aria-label={t("catalogue.pages.label")} className="flex items-center justify-between gap-2">
            <Button
              variant="secondary"
              size="sm"
              disabled={answer.page <= 1}
              disabledReason={t("catalogue.pages.first")}
              onClick={() => onSearch({ ...search, page: answer.page - 1 > 1 ? answer.page - 1 : undefined })}
            >
              {t("catalogue.pages.previous")}
            </Button>
            <span className="text-body text-fg-muted">
              {t("catalogue.pages.of", { page: answer.page, pages })}
            </span>
            <Button
              variant="secondary"
              size="sm"
              disabled={answer.page >= pages}
              disabledReason={t("catalogue.pages.last")}
              onClick={() => onSearch({ ...search, page: answer.page + 1 })}
            >
              {t("catalogue.pages.next")}
            </Button>
          </nav>
        ) : null}
      </section>
    </div>
  );
}

/** The share of the listed datasets a value must cover for its filter to count as narrowing nothing. */
const IDLE_SHARE = 0.95;

/** How many options a facet shows before "Show all"; a facet longer than SEARCHABLE gets a search field. */
const FIRST_OPTIONS = 6;
const SEARCHABLE = 10;

/** The name of one facet value in the reader's words: a theme's name, a region's, else its label. */
function useOptionLabel(): (facet: FacetName, value: string, label: string) => string {
  const themeLabel = useThemeLabel();
  return (facet, value, label) =>
    facet === "theme" ? themeLabel(value, label) : facet === "spatial" ? areaLabel(value) : label;
}

/** The chosen filters as removable chips above the results, with "Clear all" (T-7317). */
function ChosenFilters({
  search,
  answer,
  onSearch,
  onClear,
}: {
  search: CatalogueSearch;
  answer: CataloguePageAnswer;
  onSearch: (next: CatalogueSearch) => void;
  onClear: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const optionLabel = useOptionLabel();
  return (
    <div role="group" aria-label={t("catalogue.facets.chosen")} className="flex flex-wrap items-center gap-2">
      {FACETS.flatMap((facet) =>
        (search[facet] ?? []).map((value) => {
          const label = optionLabel(facet, value, answer.facets[facet].find((v) => v.value === value)?.label ?? value);
          return (
            <Button
              key={`${facet}:${value}`}
              variant="secondary"
              size="sm"
              aria-label={t("catalogue.facets.remove", { facet: t(`catalogue.facets.${facet}`), value: label })}
              onClick={() => onSearch(toggled(search, facet, value))}
            >
              <span className="text-fg-muted">{t(`catalogue.facets.${facet}`)}:</span> {label}
              <Icon name="close" className="size-3.5" />
            </Button>
          );
        }),
      )}
      <Button variant="ghost" size="sm" onClick={onClear}>
        {t("catalogue.facets.clearAll")}
      </Button>
    </div>
  );
}

/**
 * One filter as its own collapsible section (T-7317): open for Publisher and Theme or while one of
 * its values is ticked; the first options in place and "Show all N" for the rest, never a scroll
 * box inside the sidebar; a search field once it is long; each count a badge at the right edge.
 */
function FacetGroup({
  facet,
  values,
  selected,
  idle,
  onToggle,
}: {
  facet: FacetName;
  values: FacetValue[];
  selected: string[];
  /** Every listed dataset already has each value: the filter narrows nothing. */
  idle: boolean;
  onToggle: (value: string) => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  const optionLabel = useOptionLabel();
  const [all, setAll] = useState(false);
  const [needle, setNeedle] = useState("");
  // A selected value the current search no longer counts stays, so it can be turned off again.
  const options = [
    ...values,
    ...selected
      .filter((value) => !values.some((v) => v.value === value))
      .map((value) => ({ value, label: value, count: 0 })),
  ].map((v) => ({ ...v, label: optionLabel(facet, v.value, v.label) }));
  if (options.length === 0) {
    return null;
  }
  const word = needle.trim().toLocaleLowerCase();
  const matching = word ? options.filter((v) => v.label.toLocaleLowerCase().includes(word)) : options;
  // The ticked ones stay in sight when the list is cut.
  const shown =
    all || word
      ? matching
      : [...matching.filter((v) => selected.includes(v.value)), ...matching.filter((v) => !selected.includes(v.value))].slice(0, FIRST_OPTIONS);
  const name = t(`catalogue.facets.${facet}`);
  return (
    <details
      open={!idle && (facet === "publisher" || facet === "theme" || selected.length > 0) ? true : undefined}
      className="group rounded-lg border border-border bg-surface"
    >
      <summary className="flex cursor-pointer list-none items-center justify-between gap-2 px-4 py-3 text-body font-semibold text-fg [&::-webkit-details-marker]:hidden">
        <span>
          {name}
          {selected.length > 0 ? <Badge tone="primary" className="ml-2">{selected.length}</Badge> : null}
        </span>
        <Icon name="chevronDown" className="size-4 text-fg-muted transition-transform group-open:rotate-180" />
      </summary>
      <fieldset className="flex flex-col gap-2 border-t border-border px-4 pb-4 pt-3">
        <legend className="sr-only">{name}</legend>
        {idle ? <p className="text-caption text-fg-muted">{t("catalogue.facets.everywhere")}</p> : null}
        {options.length > SEARCHABLE ? (
          <Input
            className="mb-1"
            type="search"
            value={needle}
            onChange={(event) => setNeedle(event.target.value)}
            aria-label={t("catalogue.facets.find.label", { facet: name })}
            placeholder={t("catalogue.facets.find.placeholder", { example: options[0]?.label ?? "" })}
          />
        ) : null}
        {shown.map((value) => (
          <Checkbox
            key={value.value}
            className="flex w-full items-start [&>span:first-of-type]:min-w-0 [&>span:first-of-type]:flex-1"
            checked={selected.includes(value.value)}
            onChange={() => onToggle(value.value)}
            label={<span className="break-words" title={facet === "spatial" ? value.value : undefined}>{value.label}</span>}
            hint={<Badge tone="neutral" className="tabular-nums">{value.count}</Badge>}
          />
        ))}
        {word && matching.length === 0 ? <p className="text-caption text-fg-muted">{t("catalogue.facets.noMatch")}</p> : null}
        {!word && matching.length > FIRST_OPTIONS ? (
          <Button variant="ghost" size="sm" className="self-start" aria-expanded={all} onClick={() => setAll(!all)}>
            {all ? t("catalogue.facets.fewer") : t("catalogue.facets.all", { count: matching.length })}
          </Button>
        ) : null}
      </fieldset>
    </details>
  );
}

/** The formats a person picks a dataset by, first: a file, a map, the API, an assistant (T-4995). */
const PREFERRED_FORMATS = ["CSV", "OGCFEAT", "NGSI-LD", "MCP"];
const KEY_FORMATS_SHOWN = 4;

/** The card's few formats: the preferred ones it has, then the others in the catalogue's order. */
export function keyFormats(formats: string[]): string[] {
  const rank = (format: string) => {
    const at = PREFERRED_FORMATS.indexOf(format);
    return at === -1 ? PREFERRED_FORMATS.length : at;
  };
  return [...formats].sort((a, b) => rank(a) - rank(b)).slice(0, KEY_FORMATS_SHOWN);
}

function DatasetCard({ dataset }: { dataset: CatalogueDataset }): JSX.Element {
  const { t, i18n } = useTranslation();
  const themeLabel = useThemeLabel();
  const modified = dataset.modified ? new Date(dataset.modified) : null;
  const shown = keyFormats(dataset.formats);
  const hidden = dataset.formats.filter((format) => !shown.includes(format));
  return (
    <Card className="flex flex-col gap-2">
      <h2 className="font-heading text-title font-semibold">
        <Link
          to="/catalogue/$name"
          params={{ name: dataset.name }}
          className="focus-ring rounded-sm text-fg hover:underline"
        >
          {dataset.title}
        </Link>
      </h2>
      <p className="text-caption text-fg-muted">
        {[
          dataset.publisher?.title,
          dataset.licence?.title,
          modified && !Number.isNaN(modified.getTime())
            ? t("catalogue.dataset.modified", {
                date: modified.toLocaleDateString(i18n.resolvedLanguage ?? i18n.language),
              })
            : null,
        ]
          .filter(Boolean)
          .join(" · ")}
      </p>
      {dataset.notes ? <p className="line-clamp-3 text-body text-fg">{dataset.notes}</p> : null}
      {dataset.formats.length > 0 || dataset.themes.length > 0 ? (
        <ul className="flex flex-wrap gap-1" aria-label={t("catalogue.dataset.tags")}>
          {dataset.themes.map((theme) => (
            <li key={`theme-${theme}`}>
              <Badge tone="info">{themeLabel(theme)}</Badge>
            </li>
          ))}
          {shown.map((format) => (
            <li key={`format-${format}`}>
              <Badge mono>{format}</Badge>
            </li>
          ))}
          {hidden.length > 0 ? (
            <li>
              {/* The rest a hover away; the dataset's own page lists every one. */}
              <Badge title={hidden.join(", ")}>
                {t("catalogue.dataset.moreFormats", { count: hidden.length })}
                <span className="sr-only">: {hidden.join(", ")}</span>
              </Badge>
            </li>
          ) : null}
        </ul>
      ) : null}
    </Card>
  );
}

