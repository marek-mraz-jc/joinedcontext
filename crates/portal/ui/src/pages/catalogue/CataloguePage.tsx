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
  const groups = FACETS.map((facet) => (
    <FacetGroup
      key={facet}
      facet={facet}
      values={answer.facets[facet]}
      selected={search[facet] ?? []}
      onToggle={(value) => onSearch(toggled(search, facet, value))}
    />
  ));
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
        footer={
          <Button variant="primary" onClick={() => setFiltersOpen(false)}>
            {t("catalogue.facets.show", { count: answer.total })}
          </Button>
        }
      >
        <div className="flex flex-col gap-4">{groups}</div>
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
            size="sm"
            className="md:hidden"
            aria-haspopup="dialog"
            onClick={() => setFiltersOpen(true)}
          >
            {t("catalogue.facets.open", { count: chosen })}
          </Button>
        </div>
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

function FacetGroup({
  facet,
  values,
  selected,
  onToggle,
}: {
  facet: FacetName;
  values: FacetValue[];
  selected: string[];
  onToggle: (value: string) => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  const themeLabel = useThemeLabel();
  // A selected value the current search no longer counts stays, so it can be turned off again.
  const shown = [
    ...values,
    ...selected
      .filter((value) => !values.some((v) => v.value === value))
      .map((value) => ({ value, label: value, count: 0 })),
  ];
  if (shown.length === 0) {
    return null;
  }
  return (
    <fieldset className="flex flex-col gap-1.5">
      <legend className="mb-1 text-caption font-semibold uppercase tracking-wide text-fg-muted">
        {t(`catalogue.facets.${facet}`)}
      </legend>
      <div className="flex max-h-56 flex-col gap-1.5 overflow-y-auto">
        {shown.map((value) => (
          <Checkbox
            key={value.value}
            checked={selected.includes(value.value)}
            onChange={() => onToggle(value.value)}
            label={
              facet === "theme" ? (
                themeLabel(value.value, value.label)
              ) : facet === "spatial" ? (
                // The region's name, the code a hover away (T-4160 #13).
                <span title={value.value}>{areaLabel(value.value)}</span>
              ) : (
                value.label
              )
            }
            hint={`(${value.count})`}
          />
        ))}
      </div>
    </fieldset>
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

