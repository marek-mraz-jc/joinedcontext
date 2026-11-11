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
import { Button } from "../components/ui";
import type { DataLink, RowCard } from "../components/entities/EndpointDataView";

export interface MountOptions {
  /** The gateway's origin, the only one the widget reads: `https://api.{domain}` on dev and prod,
   * jc's own origin when its viz module serves the page (named by the server, never the address). */
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
  /** The page's own actions (Data API, Embed), drawn first in the view's action group in its look;
   * the click hands over the button, where the page returns focus when its dialog closes. */
  actions?: PageAction[];
}

export interface PageAction {
  /** The button's id, unique on the page. */
  id: string;
  label: string;
  onClick: (button: HTMLButtonElement) => void;
  /** The button opens a dialog. */
  hasPopup?: boolean;
}

const SLUG = /^[a-z0-9-]{1,64}$/;
// Any https origin, or http on loopback (a local jc): a scheme, a host and a port, nothing else.
const ORIGIN = /^(https:\/\/[a-z0-9]([a-z0-9.-]*[a-z0-9])?|http:\/\/(localhost|127\.0\.0\.1|\[::1\]))(:[0-9]{1,5})?$/;

export function mount(element: Element, options: MountOptions): Root {
  // The page passes its own host's sibling; anything else is a mistake to refuse, never to follow.
  if (!ORIGIN.test(options.origin)) throw new Error(`jcExplorer: origin ${options.origin} is no https origin (or http on loopback)`);
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
              // The page's frame grows to its rows, so a sheet at its bottom would be off screen (T-6748).
              anchorMenu
              actions={options.actions?.map((action) => (
                <Button
                  key={action.id}
                  id={action.id}
                  size="sm"
                  variant="secondary"
                  aria-haspopup={action.hasPopup ? "dialog" : undefined}
                  onClick={(event) => action.onClick(event.currentTarget)}
                >
                  {action.label}
                </Button>
              ))}
            />
          </div>
        </QueryClientProvider>
      </I18nextProvider>
    </StrictMode>,
  );
  return root;
}
