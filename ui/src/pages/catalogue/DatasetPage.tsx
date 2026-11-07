import { Suspense, lazy, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { useAuthStatus } from "../../auth/AuthProvider";
import { ApiError } from "../../api/client";
import type { components } from "../../api/schema";
import { CodeBlock,
  Badge,
  Button,
  Card,
  CardHeader,
  ExternalLink,
  Icon,
  PageFailed,
  PageHeader,
  PageLoading,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { useThemeLabel } from "./labels";
import { snippets } from "./snippets";
import type { Snippet } from "./snippets";
import { areaLabel } from "./nuts";
import { QualityScore } from "../../components/quality/QualityScore";
import { datasetQuery } from "./queries";
import { subscriptionsOf } from "./subscriptions";

// The menu's dropdown (Radix, about 30 KB gz) loads beside the page, not in its first chunk, and only
// for a dataset with something to subscribe to (T-5165).
// The map loads beside the page, and MapLibre only for a dataset with places (T-5628).
const DatasetMap = lazy(() => import("./DatasetMap").then((module) => ({ default: module.DatasetMap })));
// The data grid (SDK EntityGrid, T-6376) loads beside the page, as on the CKAN explorer.
const EndpointDataView = lazy(() =>
  import("../../components/entities/EndpointDataView").then((module) => ({ default: module.EndpointDataView })),
);
const SubscribeMenu = lazy(() => import("./SubscribeMenu").then((module) => ({ default: module.SubscribeMenu })));

type DatasetDetail = components["schemas"]["CatalogueDatasetDetail"];

/**
 * One public dataset (EP-82, Architecture/21 §6): what the catalogue says of it, its resources,
 * and — when it describes an Endpoint of this installation — the model's classes, ten rows read
 * through that Endpoint, and how to use the data from a shell, an App or an MCP client.
 */
export function DatasetPage({ name }: { name: string }): JSX.Element {
  const { t } = useTranslation();
  const dataset = useQuery(datasetQuery(name));

  const back = (
    <Link to="/catalogue" className="focus-ring inline-flex items-center gap-1 rounded-sm text-body text-primary-soft-fg hover:underline">
      <Icon name="chevronLeft" className="size-4" />
      {t("catalogue.dataset.back")}
    </Link>
  );

  if (dataset.isPending) {
    return (
      <div className="flex flex-col gap-section">
        {back}
        <PageLoading label={t("catalogue.dataset.loading")} />
      </div>
    );
  }
  if (dataset.isError) {
    const missing = dataset.error instanceof ApiError && dataset.error.status === 404;
    return (
      <div className="flex flex-col gap-section">
        {back}
        {/* Every state of the page is a page with its heading: a person who followed an old link
            learns at once what is not there. */}
        {missing ? (
          <PageHeader title={t("catalogue.dataset.missing")} description={t("catalogue.dataset.missingHint")} />
        ) : (
          <>
            <PageHeader title={t("catalogue.title")} />
            <PageFailed
              error={dataset.error}
              onRetry={() => {
                void dataset.refetch();
              }}
            />
          </>
        )}
      </div>
    );
  }
  return <Dataset detail={dataset.data} back={back} />;
}

function Dataset({ detail, back }: { detail: DatasetDetail; back: JSX.Element }): JSX.Element {
  const { t, i18n } = useTranslation();
  const themeLabel = useThemeLabel();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const date = (value: string | null | undefined) => {
    const parsed = value ? new Date(value) : null;
    return parsed && !Number.isNaN(parsed.getTime()) ? parsed.toLocaleDateString(locale) : null;
  };
  const temporal = detail.temporal
    ? t("catalogue.dataset.period", {
        start: date(detail.temporal.start) ?? t("catalogue.dataset.open"),
        end: date(detail.temporal.end) ?? t("catalogue.dataset.open"),
      })
    : null;
  const frequency = detail.frequency ? detail.frequency.split("/").pop() : null;
  const facts: Array<[string, React.ReactNode]> = [
    [t("catalogue.facets.publisher"), detail.publisher?.title],
    [
      t("catalogue.facets.licence"),
      detail.licence ? (
        detail.licence.url ? (
          <ExternalLink href={detail.licence.url}>{detail.licence.title}</ExternalLink>
        ) : (
          detail.licence.title
        )
      ) : null,
    ],
    [
      t("catalogue.dataset.frequency"),
      frequency ? t(`catalogue.frequency.${frequency}`, { defaultValue: frequency }) : null,
    ],
    [
      t("catalogue.facets.spatial"),
      detail.spatial.length > 0 ? (
        <ul className="flex flex-wrap gap-x-2">
          {detail.spatial.map((area) => (
            <li key={area} title={area}>
              {areaLabel(area)}
            </li>
          ))}
        </ul>
      ) : null,
    ],
    [t("catalogue.facets.year"), temporal],
    [
      t("catalogue.dataset.contact"),
      detail.contact
        ? [detail.contact.name, detail.contact.email].filter(Boolean).join(", ")
        : null,
    ],
    [t("catalogue.dataset.modifiedLabel"), date(detail.modified)],
  ];

  // The notes are the publisher's Markdown: the first paragraph says what the dataset is.
  const blocks = notesBlocks(detail.notes ?? "");
  const lead = blocks[0]?.code === false ? blocks[0] : undefined;
  const rest = lead ? blocks.slice(1) : blocks;
  const developer = developerSlug(detail.endpoint?.url);

  return (
    <div className="flex flex-col gap-section">
      {back}
      <PageHeader
        title={detail.title}
        description={lead ? <Inline text={lead.text} /> : undefined}
        actions={
          <div className="flex flex-wrap items-center gap-3">
            {detail.endpoint && hasSubscriptions(detail.endpoint) ? (
              <Suspense
                fallback={
                  <Button variant="secondary" disabled title={t("app.loading")}>
                    <Icon name="share" className="size-4" />
                    {t("catalogue.subscribe.button")}
                  </Button>
                }
              >
                <SubscribeMenu endpoint={detail.endpoint} />
              </Suspense>
            ) : null}
            <ExternalLink href={detail.catalogueUrl}>{t("catalogue.dataset.inCatalogue")}</ExternalLink>
          </div>
        }
      />
      {rest.length > 0 ? <Blocks blocks={rest} className="max-w-prose text-body text-fg-muted" /> : null}

      <Card>
        <dl className="grid gap-x-6 gap-y-3 sm:grid-cols-2">
          {facts
            .filter(([, value]) => value !== null && value !== undefined && value !== "")
            .map(([label, value]) => (
              <div key={label} className="flex min-w-0 flex-col gap-0.5">
                <dt className="text-caption font-semibold text-fg-muted">{label}</dt>
                {/* A NUTS URI is one long word: it breaks where it must instead of widening the
                    page (T-4265, photo audit T-4160 #40). */}
                <dd className="text-body text-fg [overflow-wrap:anywhere]">{value}</dd>
              </div>
            ))}
        </dl>
        {detail.themes.length > 0 || detail.keywords.length > 0 ? (
          <ul className="mt-4 flex flex-wrap gap-1" aria-label={t("catalogue.dataset.tags")}>
            {detail.themes.map((theme) => (
              <li key={`theme-${theme.code}`}>
                <Badge tone="info">{themeLabel(theme.code, theme.label)}</Badge>
              </li>
            ))}
            {detail.keywords.map((keyword) => (
              <li key={`keyword-${keyword}`}>
                <Badge>{keyword}</Badge>
              </li>
            ))}
          </ul>
        ) : null}
      </Card>

      {detail.quality ? <QualityScore score={detail.quality} /> : null}
      {detail.endpoint ? (
        <Suspense fallback={null}>
          <DatasetMap name={detail.name} />
        </Suspense>
      ) : null}
      <Resources detail={detail} />
      {detail.model ? <Model model={detail.model} /> : null}
      {detail.endpoint ? <DatasetData endpoint={detail.endpoint} /> : null}
      {developer ? (
        <a className="w-fit text-body text-primary-soft-fg underline" href={`/d/${developer}`}>
          {t("catalogue.dataset.developerPage")}
        </a>
      ) : null}
      {detail.endpoint ? (
        <UseThisData
          items={snippets(
            detail.endpoint.url,
            detail.endpoint.representations,
            detail.model?.classes[0]?.name,
            detail.name,
          )}
        />
      ) : null}
    </div>
  );
}

function hasSubscriptions(endpoint: NonNullable<DatasetDetail["endpoint"]>): boolean {
  const { feeds, calendars } = subscriptionsOf(endpoint);
  return feeds.length > 0 || calendars.length > 0;
}

/** The slug of a public Endpoint address, the name of its developer page `/d/{slug}` (T-5111). */
export function developerSlug(url: string | undefined): string | undefined {
  return /\/api\/endpoint\/([a-z0-9]{16,64})\//.exec(url ?? "")?.[1];
}

/** The https origin of an Endpoint URL the catalogue gives, or nothing for any other URL. */
export function dataOrigin(url: string): string | undefined {
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" ? parsed.origin : undefined;
  } catch {
    return undefined;
  }
}

function Resources({ detail }: { detail: DatasetDetail }): JSX.Element {
  const { t } = useTranslation();
  return (
    <section aria-labelledby="catalogue-resources" className="flex flex-col gap-3">
      <h2 id="catalogue-resources" className="font-heading text-title font-semibold">
        {t("catalogue.dataset.resources")}
      </h2>
      {detail.resources.length === 0 ? (
        <p className="text-body text-fg-muted">{t("catalogue.dataset.noResources")}</p>
      ) : (
        <Table caption={t("catalogue.dataset.resources")}>
          <TableHead>
            <TableHeaderCell>{t("catalogue.dataset.resource")}</TableHeaderCell>
            <TableHeaderCell>{t("catalogue.facets.format")}</TableHeaderCell>
            <TableHeaderCell>{t("catalogue.dataset.links")}</TableHeaderCell>
          </TableHead>
          <TableBody>
            {detail.resources.map((resource) => (
              <TableRow key={`${resource.format}-${resource.url}`}>
                <TableCell primary>
                  <div>{resource.name}</div>
                  <ResourceSays format={resource.format} />
                  {/* The publisher's text is commands and digests for a developer, in English: it
                      waits behind a disclosure, the sentence above says what the file is (T-4160 #16). */}
                  {resource.description ? (
                    <details className="mt-0.5 text-caption">
                      <summary className="cursor-pointer text-fg-muted">{t("catalogue.dataset.forDevelopers")}</summary>
                      <Blocks blocks={notesBlocks(resource.description)} className="mt-1 text-fg-muted [overflow-wrap:anywhere]" />
                    </details>
                  ) : null}
                </TableCell>
                <TableCell>
                  <Badge mono>{resource.format || "—"}</Badge>
                </TableCell>
                <TableCell>
                  <ul className="flex flex-wrap gap-3">
                    <li>
                      <ExternalLink href={resource.url}>{t("catalogue.dataset.download")}</ExternalLink>
                    </li>
                  </ul>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </section>
  );
}

/** The formats a schema artifact is published in (jcctl `format_of`). */
const MODEL_FORMATS = new Set(["linkml", "jsonschema", "jsonld", "shacl", "owl", "rdf", "markdown"]);

/**
 * One sentence in the page's language of what a resource is, by its CKAN format (T-4264, T-4160
 * #16): "Tabuľka CSV na stiahnutie". A format the catalogue has no sentence for says nothing.
 */
/** One block of a publisher's Markdown text: a paragraph, or an indented code block. */
export interface NotesBlock {
  code: boolean;
  text: string;
}

/**
 * The publisher's Markdown (CKAN notes and resource descriptions, written by jcctl publish) in
 * the two shapes it uses: paragraphs with `code` spans, and blocks indented by four spaces that
 * hold commands and JSON (T-4949). Nothing is parsed as HTML: every block is rendered as text.
 */
export function notesBlocks(text: string): NotesBlock[] {
  return text
    .replace(/\r\n/g, "\n")
    .split(/\n[ \t]*\n/)
    .filter((block) => block.trim() !== "")
    .map((block) => {
      const lines = block.split("\n").filter((line) => line.trim() !== "");
      const code = lines.every((line) => /^( {4}|\t)/.test(line));
      return code
        ? { code, text: lines.map((line) => line.replace(/^( {4}|\t)/, "")).join("\n") }
        : { code, text: block.trim() };
    });
}

/** A paragraph with its `code` spans in code. */
function Inline({ text }: { text: string }): JSX.Element {
  return (
    <>
      {text.split(/(`[^`\n]+`)/).map((part, index) =>
        /^`[^`]+`$/.test(part) ? <code key={index}>{part.slice(1, -1)}</code> : part,
      )}
    </>
  );
}

function Blocks({ blocks, className }: { blocks: NotesBlock[]; className?: string }): JSX.Element {
  return (
    <div className={`flex flex-col gap-2 ${className ?? ""}`}>
      {blocks.map((block, index) =>
        block.code ? (
          <pre key={index} className="whitespace-pre-wrap rounded bg-surface-muted p-2 text-caption [overflow-wrap:anywhere]">
            <code>{block.text}</code>
          </pre>
        ) : (
          <p key={index} className="whitespace-pre-line">
            <Inline text={block.text} />
          </p>
        ),
      )}
    </div>
  );
}

export function resourceSays(format: string, t: (key: string, options?: Record<string, unknown>) => string): string | null {
  const key = format.toLowerCase().replace(/[^a-z0-9]/g, "");
  if (MODEL_FORMATS.has(key)) return t("catalogue.dataset.says.model", { format });
  return key && key !== "model" ? t(`catalogue.dataset.says.${key}`, { defaultValue: "" }) || null : null;
}

function ResourceSays({ format }: { format: string }): JSX.Element | null {
  const { t } = useTranslation();
  const says = resourceSays(format, t);
  return says ? <div className="mt-0.5 text-caption text-fg-muted">{says}</div> : null;
}

function Model({ model }: { model: NonNullable<DatasetDetail["model"]> }): JSX.Element {
  const { t } = useTranslation();
  return (
    <Card>
      <CardHeader
        title={t("catalogue.dataset.model", { name: model.name })}
        actions={
          model.docsUrl ? <ExternalLink href={model.docsUrl}>{t("catalogue.dataset.modelDocs")}</ExternalLink> : undefined
        }
      />
      <dl className="flex flex-col gap-2">
        {model.classes.map((cls) => (
          <div key={cls.name}>
            <dt className="font-mono text-body font-medium text-fg">{cls.name}</dt>
            <dd className="text-body text-fg-muted">{cls.description ?? t("catalogue.dataset.noDescription")}</dd>
          </div>
        ))}
      </dl>
    </Card>
  );
}

/**
 * The dataset's entities in the one data grid every page uses (T-6376): read from the browser
 * through the Endpoint with the visitor's own session, so the Endpoint's Policy decides what each
 * person sees, and a private dataset says sign in or no access rather than drawing an empty grid.
 */
export function DatasetData({ endpoint }: { endpoint: NonNullable<DatasetDetail["endpoint"]> }): JSX.Element | null {
  const { t } = useTranslation();
  const status = useAuthStatus();
  const slug = developerSlug(endpoint.url);
  if (!slug) return null;
  // A visitor without a session reads the Endpoint's own data host: the Portal host answers each of
  // their reads with a redirect there, one round trip more per read (T-6540). A signed-in person
  // reads through the Portal host, whose edge turns the session into their bearer.
  const origin = status === "anonymous" ? dataOrigin(endpoint.url) : undefined;
  return (
    <section aria-labelledby="catalogue-data" className="flex flex-col gap-3">
      <h2 id="catalogue-data" className="font-heading text-title font-semibold">
        {t("catalogue.dataset.data")}
      </h2>
      {status === "loading" ? (
        <p role="status">{t("app.loading")}</p>
      ) : (
        <Suspense fallback={<p role="status">{t("app.loading")}</p>}>
          <EndpointDataView project="catalogue" slug={slug} csv={endpoint.representations.includes("csv")} origin={origin} />
        </Suspense>
      )}
    </section>
  );
}

function UseThisData({ items }: { items: Snippet[] }): JSX.Element | null {
  const { t } = useTranslation();
  if (items.length === 0) {
    return null;
  }
  return (
    <section aria-labelledby="catalogue-use" className="flex flex-col gap-3">
      <h2 id="catalogue-use" className="font-heading text-title font-semibold">
        {t("catalogue.use.title")}
      </h2>
      {items.map((item) => (
        <SnippetBlock key={item.key} item={item} />
      ))}
    </section>
  );
}

function SnippetBlock({ item }: { item: Snippet }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const title = t(`catalogue.use.${item.key}`);
  return (
    <Card className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-body font-semibold text-fg">{title}</h3>
        <Button
          size="sm"
          variant="ghost"
          aria-label={copied ? t("catalogue.use.copied") : t("catalogue.use.copy", { what: title })}
          icon={<Icon name={copied ? "check" : "copy"} className="size-4" />}
          onClick={() => {
            void navigator.clipboard?.writeText(item.code).then(
              () => setCopied(true),
              () => setCopied(false),
            );
          }}
        >
          {copied ? t("catalogue.use.copied") : t("catalogue.use.copyShort")}
        </Button>
      </div>
      {/* A long line scrolls sideways, so the block takes the keyboard focus that scrolls it
          (axe scrollable-region-focusable) and is named by its heading. */}
      <CodeBlock
        label={title}
        className="overflow-x-auto rounded-md bg-surface-muted p-3 font-mono text-caption text-fg"
      >
        <code>{item.code}</code>
      </CodeBlock>
    </Card>
  );
}
