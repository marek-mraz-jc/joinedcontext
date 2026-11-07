/**
 * The catalogue's live data widget (T-5082): the Portal's Endpoint "Data" view, the same component,
 * built as one script for `viz.{domain}/explore.html`. It reads `api.{domain}` without a cookie or
 * token, so it shows what anyone may read; the Portal page shows the same view with the person's
 * session. The page's own script (`explore.js`) calls `jcExplorer.mount` and keeps the map and the
 * API addresses in step through `onLink`.
 */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "./i18n";
import "../index.css";
import "./explorer.css";
import { EndpointDataView } from "../components/entities/EndpointDataView";
import type { DataLink, RowCard } from "../components/entities/EndpointDataView";

export interface MountOptions {
  /** `https://api.{domain}`: the only origin the widget reads. */
  origin: string;
  slug: string;
  lang?: string;
  /** The type, query and order the address opens with. */
  link?: DataLink;
  onLink?: (next: DataLink) => void;
  /** The chosen type's card: labels, units and kinds for the page's own row card (DM-135). */
  onCard?: (card: RowCard) => void;
  /** A row the reader opened: the page shows it in its own card (DM-135). */
  onOpenRow?: (id: string) => void;
  /** The endpoint serves `file.csv`. */
  csv?: boolean;
}

const SLUG = /^[a-z0-9-]{1,64}$/;
const ORIGIN = /^https:\/\/api\.[a-z0-9.-]+$/;

export function mount(element: Element, options: MountOptions): Root {
  // The page passes its own host's sibling; anything else is a mistake to refuse, never to follow.
  if (!ORIGIN.test(options.origin)) throw new Error(`jcExplorer: origin ${options.origin} is not https://api.{domain}`);
  if (!SLUG.test(options.slug)) throw new Error("jcExplorer: the slug names no Endpoint");
  const lang = options.lang === "sk" ? "sk" : "en";
  void i18n.changeLanguage(lang);
  const client = new QueryClient({ defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } } });
  const root = createRoot(element);
  root.render(
    <StrictMode>
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={client}>
          <div className="jc-explorer">
            <EndpointDataView
              project="catalogue"
              slug={options.slug}
              origin={options.origin}
              link={options.link}
              onLink={options.onLink}
              onCard={options.onCard}
              onOpenRow={options.onOpenRow}
              csv={options.csv ?? true}
            />
          </div>
        </QueryClientProvider>
      </I18nextProvider>
    </StrictMode>,
  );
  return root;
}
