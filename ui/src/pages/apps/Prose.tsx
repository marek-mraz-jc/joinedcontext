import { useState } from "react";
import type { JSX, MouseEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { CodeBlock, Button, ExternalLink, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../../components/ui";
import { Icon } from "../../components/ui/icons";
import { hostToShow, safeHref } from "../../components/ui/safeHref";

/**
 * The Markdown an answer is written in, as the blocks the transcript draws (T-2773, UI-84).
 *
 * A subset, the one the assistant's answers use: headings, paragraphs, lists, tables and code.
 * The model's text only ever becomes React children and checked links, never markup, so a
 * `<script>` or an `onerror` in an answer is text on the screen (AP-53).
 */
export type Block =
  | { kind: "heading"; text: string }
  | { kind: "paragraph"; text: string }
  | { kind: "list"; ordered: boolean; items: string[] }
  | { kind: "code"; text: string }
  | { kind: "table"; head: string[]; rows: string[][] };

const HEADING = /^#{1,6}\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
const NUMBERED = /^\s*\d+[.)]\s+(.*)$/;
const TABLE_RULE = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;

/** One table row's cells: the outer pipes dropped, an escaped `\|` kept inside its cell. */
function cellsOf(line: string): string[] {
  const inner = line.trim().replace(/^\|/, "").replace(/(?<!\\)\|$/, "");
  return inner.split(/(?<!\\)\|/).map((cell) => cell.replace(/\\\|/g, "|").trim());
}

export function blocksOf(text: string): Block[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  const endParagraph = () => {
    if (paragraph.length > 0) {
      blocks.push({ kind: "paragraph", text: paragraph.join("\n") });
      paragraph = [];
    }
  };
  for (let at = 0; at < lines.length; at++) {
    const line = lines[at];
    if (line.trimStart().startsWith("```")) {
      endParagraph();
      const code: string[] = [];
      at++;
      while (at < lines.length && !lines[at].trimStart().startsWith("```")) {
        code.push(lines[at]);
        at++;
      }
      blocks.push({ kind: "code", text: code.join("\n") });
      continue;
    }
    if (line.trim() === "") {
      endParagraph();
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      endParagraph();
      blocks.push({ kind: "heading", text: heading[1].trim() });
      continue;
    }
    if (line.includes("|") && at + 1 < lines.length && TABLE_RULE.test(lines[at + 1])) {
      endParagraph();
      const head = cellsOf(line);
      const rows: string[][] = [];
      at += 2;
      while (at < lines.length && lines[at].includes("|") && lines[at].trim() !== "") {
        rows.push(cellsOf(lines[at]));
        at++;
      }
      at--;
      blocks.push({ kind: "table", head, rows });
      continue;
    }
    const bullet = BULLET.exec(line);
    const numbered = bullet ? null : NUMBERED.exec(line);
    if (bullet || numbered) {
      endParagraph();
      const ordered = numbered !== null;
      const last = blocks.at(-1);
      const item = (bullet ?? numbered)?.[1] ?? "";
      if (last?.kind === "list" && last.ordered === ordered && lines[at - 1]?.trim() !== "") {
        last.items.push(item);
      } else {
        blocks.push({ kind: "list", ordered, items: [item] });
      }
      continue;
    }
    const last = blocks.at(-1);
    // An indented line under a list item goes on with that item.
    if (paragraph.length === 0 && last?.kind === "list" && /^\s{2,}\S/.test(line) && lines[at - 1]?.trim() !== "") {
      last.items[last.items.length - 1] += `\n${line.trim()}`;
      continue;
    }
    paragraph.push(line);
  }
  endParagraph();
  return blocks;
}

/** Longer than this, an id shows its type and its own last part, and a button copies it whole. */
const LONG_ID = 40;

/** `urn:ngsi-ld:Type:…:local`: the type and the entity's own name, the middle left out. */
export function shortId(urn: string): string {
  if (urn.length <= LONG_ID) return urn;
  const parts = urn.split(":");
  const local = parts.at(-1) ?? "";
  if (parts.length <= 4) return `${urn.slice(0, 24)}…${urn.slice(-12)}`;
  const type = parts.slice(0, 3).join(":");
  return local.length <= 24 ? `${type}:…:${local}` : `${type}:…${local.slice(-20)}`;
}

function LongId({ id }: { id: string }): JSX.Element {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  if (id.length <= LONG_ID) {
    return <code className="break-all font-mono">{id}</code>;
  }
  return (
    <span className="inline-flex items-baseline gap-0.5">
      <code className="font-mono" title={id}>
        <span aria-hidden>{shortId(id)}</span>
        <span className="sr-only">{id}</span>
      </code>
      <Button
        variant="ghost"
        size="xs"
        className="h-5 w-5 self-center px-0 text-fg-muted"
        aria-label={copied ? t("agentRun.prose.copied") : t("agentRun.prose.copyId")}
        title={copied ? t("agentRun.prose.copied") : t("agentRun.prose.copyId")}
        onClick={() => {
          void navigator.clipboard?.writeText(id).then(
            () => setCopied(true),
            () => setCopied(false),
          );
        }}
        icon={<Icon name={copied ? "check" : "copy"} className="size-3" />}
      />
    </span>
  );
}

/** Opens a Portal route the way the page around the answer navigates; a plain link without it. */
export type OpenLink = (href: string) => void;

export function PortalLink({ href, children, onOpenLink }: { href: string; children: ReactNode; onOpenLink?: OpenLink }): JSX.Element {
  const safe = safeHref(href);
  if (safe === undefined) {
    return <>{children}</>;
  }
  if (!safe.startsWith("/")) {
    return (
      <ExternalLink href={safe} className="focus-ring rounded-sm text-primary-soft-fg underline">
        {children}
      </ExternalLink>
    );
  }
  const open = (event: MouseEvent<HTMLAnchorElement>) => {
    // A modified click keeps the browser's own meaning: a new tab, a new window.
    if (!onOpenLink || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onOpenLink(safe);
  };
  return (
    <a href={safe} onClick={open} className="focus-ring rounded-sm text-primary-soft-fg underline">
      {children}
    </a>
  );
}

const INLINE =
  /`([^`\n]+)`|\[([^\]\n]+)\]\(([^)\s]+)\)|\*\*([^*\n]+)\*\*|(?<![\w*])\*([^*\n]+)\*(?![\w*])|(urn:ngsi-ld:[^\s)\]`|,;<>]+)|(https?:\/\/[^\s)\]<>`]+)/g;

/** A sentence's full stop after an id or an address is the sentence's, not the id's. */
function splitTrailing(token: string): [string, string] {
  const match = /[.,:;!?]+$/.exec(token);
  return match ? [token.slice(0, match.index), match[0]] : [token, ""];
}

export function inline(text: string, onOpenLink?: OpenLink): ReactNode[] {
  const nodes: ReactNode[] = [];
  let from = 0;
  let key = 0;
  for (const match of text.matchAll(INLINE)) {
    const at = match.index;
    if (at > from) nodes.push(text.slice(from, at));
    const [whole, code, label, href, bold, italic, urn, url] = match;
    if (code !== undefined) {
      nodes.push(
        code.startsWith("urn:ngsi-ld:") ? (
          <LongId key={key++} id={code} />
        ) : (
          <code key={key++} className="rounded-sm bg-surface px-1 font-mono">
            {code}
          </code>
        ),
      );
    } else if (label !== undefined && href !== undefined) {
      nodes.push(
        <PortalLink key={key++} href={href} onOpenLink={onOpenLink}>
          {inline(label, onOpenLink)}
        </PortalLink>,
      );
      // A label is the model's words, and text the model read can write them: a link that
      // leaves the Portal says where it goes beside them (F-R2UI2-004).
      const host = hostToShow(label, href);
      if (host) {
        nodes.push(
          <span key={key++} className="text-fg-muted">
            {` (${host})`}
          </span>,
        );
      }
    } else if (bold !== undefined) {
      nodes.push(<strong key={key++}>{inline(bold, onOpenLink)}</strong>);
    } else if (italic !== undefined) {
      nodes.push(<em key={key++}>{inline(italic, onOpenLink)}</em>);
    } else if (urn !== undefined) {
      const [id, rest] = splitTrailing(urn);
      nodes.push(<LongId key={key++} id={id} />);
      if (rest) nodes.push(rest);
    } else if (url !== undefined) {
      const [address, rest] = splitTrailing(url);
      nodes.push(
        <PortalLink key={key++} href={address} onOpenLink={onOpenLink}>
          {address}
        </PortalLink>,
      );
      if (rest) nodes.push(rest);
    }
    from = at + whole.length;
  }
  if (from < text.length) nodes.push(text.slice(from));
  return nodes;
}

/**
 * An answer, drawn from its Markdown. Tables keep to the column and scroll inside themselves
 * rather than widening the conversation (T-2773).
 */
export function Prose({ text, onOpenLink }: { text: string; onOpenLink?: OpenLink }): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="space-y-2 break-words">
      {blocksOf(text).map((block, at) => {
        switch (block.kind) {
          case "heading":
            return (
              <p key={at} className="font-semibold">
                {inline(block.text, onOpenLink)}
              </p>
            );
          case "paragraph":
            return (
              <p key={at} className="whitespace-pre-wrap">
                {inline(block.text, onOpenLink)}
              </p>
            );
          case "list": {
            const items = block.items.map((item, n) => (
              <li key={n} className="whitespace-pre-wrap">
                {inline(item, onOpenLink)}
              </li>
            ));
            return block.ordered ? (
              <ol key={at} className="list-decimal space-y-0.5 pl-5">
                {items}
              </ol>
            ) : (
              <ul key={at} className="list-disc space-y-0.5 pl-5">
                {items}
              </ul>
            );
          }
          case "code":
            return (
              <CodeBlock label={t("codeBlock.code")} key={at} className="overflow-x-auto rounded-md bg-surface p-2 font-mono text-xs">
                <code>{block.text}</code>
              </CodeBlock>
            );
          case "table":
            return (
              <Table key={at} caption={t("agentRun.prose.table")} zebra={false} className="text-xs">
                <TableHead>
                  {block.head.map((cell, n) => (
                    <TableHeaderCell key={n} className="px-2 py-1 normal-case tracking-normal">
                      {inline(cell, onOpenLink)}
                    </TableHeaderCell>
                  ))}
                </TableHead>
                <TableBody>
                  {block.rows.map((row, r) => (
                    <TableRow key={r}>
                      {block.head.map((_, n) => (
                        <TableCell key={n} className="px-2 py-1">
                          {inline(row[n] ?? "", onOpenLink)}
                        </TableCell>
                      ))}
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            );
        }
      })}
    </div>
  );
}
