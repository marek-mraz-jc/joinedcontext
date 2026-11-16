import { Suspense, lazy, useCallback } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import type { DataLink } from "../../components/entities/EndpointDataView";
import { useTranslation } from "react-i18next";
import { useAuthStatus } from "../../auth/AuthProvider";
import { Icon, PageHeader, PageLoading } from "../../components/ui";
import { readableQuery } from "./queries";
import { fetchJson } from "../endpoints/SchemaProjectionPanel";
import { AccessBadge } from "./ReadableDatasets";

const EndpointDataView = lazy(() =>
  import("../../components/entities/EndpointDataView").then((module) => ({ default: module.EndpointDataView })),
);

/**
 * One dataset of the catalogue by its Endpoint (ADR-N-067 §2.4, T-6411), read live with the
 * person's own session. The grid answers the refusals itself (T-6376): anonymous on a
 * non-public Endpoint gets "sign in", signed in without a grant "no access"; never an empty grid
 * and never a copy.
 */
export function EndpointDatasetPage({ slug }: { slug: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const status = useAuthStatus();
  // The title and the mark come from what the person may read; the grid decides alone.
  const readable = useQuery({ ...readableQuery(), enabled: status === "authenticated" });
  // The view's type, query and order live in the address (T-7072), as on the endpoint's own page
  // (T-5082): a reload or a shared link opens the same rows. Replaced, never pushed.
  const navigate = useNavigate();
  const search = useSearch({ strict: false }) as DataLink;
  const link: DataLink = { type: search.type, q: search.q, search: search.search, sort: search.sort };
  const writeLink = useCallback(
    (next: DataLink) => {
      if (next.type === search.type && next.q === search.q && next.search === search.search && next.sort === search.sort) return;
      void navigate({ to: ".", search: { type: next.type, q: next.q, search: next.search, sort: next.sort }, replace: true });
    },
    [navigate, search.type, search.q, search.search, search.sort],
  );
  const dataset = readable.data?.datasets.find((row) => row.slug === slug);
  // The endpoint's own DCAT entry names it for a visitor too (T-7072): its title in the page's
  // language, so nobody reads "A catalogue dataset" over a table of street lights.
  const entry = useQuery({
    queryKey: ["endpoint-dcat", slug],
    enabled: !dataset,
    retry: false,
    queryFn: () => fetchJson(`${window.location.origin}/api/endpoint/${encodeURIComponent(slug)}/`),
  });
  const named = dcatTitle(entry.data, i18n.language);
  // Without the row the page cannot name the dataset: it says why instead of a bare "Dataset".
  const words =
    status !== "authenticated"
      ? { title: t("catalogue.readable.endpoint.visitorTitle"), lead: t("catalogue.readable.endpoint.visitorLead") }
      : readable.isSuccess && !dataset
        ? { title: t("catalogue.readable.endpoint.refusedTitle"), lead: t("catalogue.readable.endpoint.refusedLead") }
        : { title: t("catalogue.readable.endpoint.title"), lead: t("catalogue.readable.endpoint.lead") };
  return (
    <div className="flex flex-col gap-section">
      <Link to="/catalogue" className="focus-ring inline-flex items-center gap-1 self-start rounded-sm text-body text-primary-soft-fg hover:underline">
        <Icon name="chevronLeft" className="size-4" />
        {t("catalogue.dataset.back")}
      </Link>
      <PageHeader
        title={dataset?.title ?? dataset?.name ?? named ?? words.title}
        titleAction={dataset ? <AccessBadge dataset={dataset} /> : undefined}
        description={dataset?.description ?? words.lead}
      />
      <Suspense fallback={<PageLoading label={t("catalogue.dataset.loading")} />}>
        <EndpointDataView
          project={dataset?.project}
          slug={slug}
          csv={dataset?.representations.includes("csv") ?? false}
          link={link}
          onLink={writeLink}
        />
      </Suspense>
    </div>
  );
}

/** A DCAT entry's `dct:title` in `language`, else in English, else the first one; none when it has none. */
export function dcatTitle(entry: unknown, language: string): string | undefined {
  const raw = (entry as { "dct:title"?: unknown } | undefined)?.["dct:title"];
  const titles = (Array.isArray(raw) ? raw : [raw]).flatMap((one): { text: string; lang?: string }[] => {
    if (typeof one === "string") return [{ text: one }];
    const value = (one as { "@value"?: unknown; "@language"?: unknown } | null)?.["@value"];
    const lang = (one as { "@language"?: unknown } | null)?.["@language"];
    return typeof value === "string" && value.trim() !== "" ? [{ text: value, ...(typeof lang === "string" ? { lang } : {}) }] : [];
  });
  const base = language.split("-")[0];
  return (titles.find((one) => one.lang === base) ?? titles.find((one) => one.lang === "en") ?? titles[0])?.text;
}
