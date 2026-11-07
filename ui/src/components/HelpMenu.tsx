import { useEffect, useState } from "react";
import type { JSX } from "react";
import { useRouterState } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { askAbout } from "../assistant/state";
import { guideUrl, useBranding } from "../branding";
import { helpFor } from "../pageHelp";
import { WHATS_NEW, isUnread, lastSeen, markSeen } from "../whatsNew";
import { CodeBlock, Badge, Button, Dialog, ExternalLink, Icon, Menu, MenuContent, MenuItem, MenuLabel, MenuTrigger, safeHref } from "./ui";

/**
 * Help in the header: help for the page the person is on (T-3269), what changed for the people who
 * use the Portal with a dot until it is read (T-3271), and the glossary. The dot is this browser's:
 * it remembers the newest entry read.
 */
/** `onFeedback` adds "Send feedback", for a phone, where the header has no room for its button. */
export function HelpMenu({ onFeedback }: { onFeedback?: () => void } = {}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const branding = useBranding();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const page = helpFor(pathname);
  // The installation's own documentation site where it names one, else the pinned pages this
  // Portal serves itself at /docs (T-4063, T-4485), so every helped page leads to its guide page.
  const guide = page ? (guideUrl(branding, page.guide) ?? `/docs/${page.guide}`) : undefined;
  // The dot follows what was read; the list marks what was new when it was opened.
  const [seen, setSeen] = useState(lastSeen);
  const [readBefore, setReadBefore] = useState<string | undefined>();
  const unread = WHATS_NEW.filter((entry) => isUnread(entry, seen)).length;
  const day = new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language ?? "sk", { dateStyle: "medium" });

  const read = () => {
    setReadBefore(lastSeen());
    markSeen();
    setSeen(lastSeen());
    setOpen(true);
  };

  return (
    <>
      <Menu>
        <MenuTrigger asChild>
          <Button variant="ghost" className="relative pointer-coarse:max-sm:px-1.5" aria-label={t("help.label", { count: unread })}>
            <Icon name="info" className="size-5" />
            {unread > 0 ? (
              <span aria-hidden="true" className="absolute right-0.5 top-0.5 size-2 rounded-full bg-primary" />
            ) : null}
          </Button>
        </MenuTrigger>
        <MenuContent align="end" className="min-w-52">
          <MenuLabel>{t("help.title")}</MenuLabel>
          {page ? (
            <MenuItem onSelect={() => setHelpOpen(true)}>
              <Icon name="info" className="size-4" />
              {t("pageHelp.menu")}
            </MenuItem>
          ) : null}
          <MenuItem onSelect={read}>
            <Icon name="inbox" className="size-4" />
            {unread > 0 ? t("whatsNew.menuUnread", { count: unread }) : t("whatsNew.menu")}
          </MenuItem>
          {onFeedback ? (
            <MenuItem className="sm:hidden" onSelect={onFeedback}>
              <Icon name="chat" className="size-4" />
              {t("feedback.button")}
            </MenuItem>
          ) : null}
          <MenuItem asChild>
            <a href="/glossary">
              <Icon name="info" className="size-4" />
              {t("glossary.page.title")}
            </a>
          </MenuItem>
        </MenuContent>
      </Menu>
      {page ? (
        <Dialog
          open={helpOpen}
          onOpenChange={setHelpOpen}
          title={t(`pageHelp.${page.key}.title`)}
          description={t(`pageHelp.${page.key}.purpose`)}
          closeLabel={t("whatsNew.close")}
          footer={
            <Button
              variant="primary"
              onClick={() => {
                setHelpOpen(false);
                // The question is written for the person to send or edit; the page goes with it.
                askAbout(t("pageHelp.ask", { page: t(`pageHelp.${page.key}.title`) }));
              }}
            >
              <Icon name="chat" className="size-4" />
              {t("pageHelp.askButton")}
            </Button>
          }
        >
          <div className="flex flex-col gap-4">
            <section aria-labelledby="page-help-steps" className="flex flex-col gap-2">
              <h3 id="page-help-steps" className="text-body font-semibold text-fg">
                {t("pageHelp.steps")}
              </h3>
              <ol className="list-decimal pl-5 text-body text-fg">
                {(["one", "two", "three"] as const).map((step) => (
                  <li key={step}>{t(`pageHelp.${page.key}.${step}`)}</li>
                ))}
              </ol>
            </section>
            <GuideSection pageKey={page.key} />
            {guide ? (
              <p className="text-body">
                <ExternalLink href={guide}>{t("pageHelp.guide")}</ExternalLink>
              </p>
            ) : null}
          </div>
        </Dialog>
      ) : null}
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title={t("whatsNew.title")}
        description={t("whatsNew.lead")}
        closeLabel={t("whatsNew.close")}
      >
        <ol className="flex flex-col gap-4">
          {WHATS_NEW.map((entry) => (
            <li key={entry.key} className="flex flex-col gap-1">
              <span className="flex flex-wrap items-center gap-2 text-caption text-fg-muted">
                <time dateTime={entry.date}>{day.format(new Date(`${entry.date}T12:00:00Z`))}</time>
                {isUnread(entry, readBefore) ? <Badge tone="info">{t("whatsNew.new")}</Badge> : null}
              </span>
              <h3 className="text-body font-semibold text-fg">{t(`whatsNew.entries.${entry.key}.title`)}</h3>
              <p className="text-body text-fg-muted">{t(`whatsNew.entries.${entry.key}.body`)}</p>
              {safeHref(entry.href) ? (
                <a href={safeHref(entry.href)} className="focus-ring w-fit rounded-sm text-body text-primary-soft-fg underline hover:no-underline">
                  {t("whatsNew.open")}
                </a>
              ) : null}
            </li>
          ))}
        </ol>
      </Dialog>
    </>
  );
}

/** One User Guide section as `scripts/guide-sections.mjs` bundles it from the pinned docs commit. */
interface GuideSectionText {
  heading: string;
  blocks: ([kind: "h" | "p" | "pre", text: string] | [kind: "ul" | "ol", items: string[]])[];
}

/**
 * The page's User Guide section, bundled at build time (T-3308): loaded with the panel's first
 * opening as its own chunk, rendered as text, in English, folded under its heading.
 */
function GuideSection({ pageKey }: { pageKey: string }): JSX.Element | null {
  const { t } = useTranslation();
  const [section, setSection] = useState<GuideSectionText | null>(null);
  useEffect(() => {
    let current = true;
    void import("../generated/guideSections.json").then((bundle) => {
      const sections = bundle.default.sections as unknown as Record<string, GuideSectionText | undefined>;
      if (current) setSection(sections[pageKey] ?? null);
    });
    return () => {
      current = false;
    };
  }, [pageKey]);
  if (!section) return null;
  return (
    <details className="text-body text-fg" data-testid="page-help-guide">
      <summary className="focus-ring cursor-pointer rounded-sm font-semibold">
        {t("pageHelp.fromGuide")} <span lang="en">{section.heading}</span>
      </summary>
      <div lang="en" className="mt-2 flex flex-col gap-2">
        {section.blocks.map((block, at) => {
          const [kind, body] = block;
          if (kind === "ul" || kind === "ol") {
            const List = kind;
            return (
              <List key={at} className={`${kind === "ol" ? "list-decimal" : "list-disc"} pl-5`}>
                {(body as string[]).map((item, n) => (
                  <li key={n}>{item}</li>
                ))}
              </List>
            );
          }
          if (kind === "h") return <h4 key={at} className="font-semibold">{body}</h4>;
          if (kind === "pre") return <CodeBlock label={t("codeBlock.code")} key={at} className="overflow-x-auto rounded bg-surface-subtle p-2 font-mono text-caption">{body}</CodeBlock>;
          return <p key={at}>{body}</p>;
        })}
      </div>
    </details>
  );
}
