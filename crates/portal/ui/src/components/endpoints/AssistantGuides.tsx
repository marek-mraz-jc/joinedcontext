import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { ExternalLink } from "../ui";

/**
 * The sections of the open data portal's one `/connector` page (T-7030), each the setup steps of
 * one assistant. The Portal keeps no copy of those clicks (T-7043): it links them.
 */
export const GUIDE_IDS = ["claude", "chatgpt", "grok", "other"] as const;
export type GuideId = (typeof GUIDE_IDS)[number];

/** Each assistant under its maker's own name, which no language translates. */
const NAMES: Record<Exclude<GuideId, "other">, string> = { claude: "Claude", chatgpt: "ChatGPT", grok: "Grok" };

type Addresses = components["schemas"]["OrganizationAddresses"];

/**
 * The first open data portal the deployment routes for the person's Organizations, as the host
 * registry lists it; undefined when it lists none. Every Organization's catalogue serves the same
 * guide, so the first one answers for all of them.
 */
export function opendataOf(addresses: Addresses | undefined): string | undefined {
  return addresses?.items.flatMap((org) => org.addresses).find((one) => one.kind === "opendata")?.url;
}

/** `{opendata}/connector#{id}`, or undefined when `opendata` is not an http(s) URL. */
export function guideUrl(opendata: string, id: GuideId): string | undefined {
  try {
    const url = new URL(opendata);
    if (url.protocol !== "https:" && url.protocol !== "http:") return undefined;
    url.pathname = `${url.pathname.replace(/\/$/, "")}/connector`;
    url.search = "";
    url.hash = id;
    return url.href;
  } catch {
    return undefined;
  }
}

/**
 * The open data host of the person's Organizations, from `GET /organization/addresses`. A stranger
 * gets a 401 and so no host; the caller then shows the address alone.
 */
export function useOpendataHost(): string | undefined {
  const addresses = useQuery({
    queryKey: ["organizationAddresses"],
    queryFn: async () => unwrap(await api.GET("/api/v1/organization/addresses")),
    retry: false,
    staleTime: 5 * 60_000,
  });
  return opendataOf(addresses.data);
}

/**
 * Where to click in each assistant: one link per section of the open data `/connector` page.
 * `signIn` says the server asks the person to sign in, which those sections cover in one line.
 */
export function AssistantGuides({ signIn = false }: { signIn?: boolean }): JSX.Element {
  const { t } = useTranslation();
  const opendata = useOpendataHost();
  const links = opendata
    ? GUIDE_IDS.flatMap((id) => {
        const href = guideUrl(opendata, id);
        return href ? [{ id, href }] : [];
      })
    : [];
  return (
    <div className="flex flex-col gap-1 rounded border border-border p-3">
      <p className="text-body text-fg">{t(links.length > 0 ? "assistantGuides.lead" : "assistantGuides.noHost")}</p>
      {links.length > 0 ? (
        <ul aria-label={t("assistantGuides.list")} className="flex flex-wrap gap-x-4">
          {links.map((link) => (
            <li key={link.id}>
              {/* min-h-11: a 44 px target a thumb hits at phone width. */}
              <ExternalLink
                href={link.href}
                className="inline-flex min-h-11 items-center text-body text-primary-soft-fg underline hover:no-underline"
              >
                {link.id === "other" ? t("assistantGuides.other") : NAMES[link.id]}
              </ExternalLink>
            </li>
          ))}
        </ul>
      ) : null}
      {signIn ? <p className="text-caption text-fg-muted">{t("assistantGuides.signIn")}</p> : null}
    </div>
  );
}
