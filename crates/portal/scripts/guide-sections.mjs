// The User Guide section each page's help panel shows (T-3308), bundled at build time from a
// pinned docs commit: no runtime fetch, no CSP or CORS change, and the words match this Portal's
// version. The sections are reduced to plain text blocks (headings, paragraphs, lists, code) so
// the panel renders them as React text, never as HTML.
//
//   node scripts/guide-sections.mjs <docs-clone> <commit>   rewrites ui/src/generated/guideSections.json
//
// The docs repository is private, so CI does not re-derive the file; a person moves the pin by
// running this against a newer docs commit and committing the result.
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/** Each helped page's guide file (as `pageHelp.ts` names it, plus `.md`) and its H2 section number. */
export const SECTIONS = {
  home: ["User-Guide/01-getting-started.md", 1],
  organization: ["User-Guide/12-managing-organization-and-projects.md", 1],
  spaces: ["User-Guide/02-organizations-projects-spaces.md", 3],
  models: ["User-Guide/03-data-models.md", 3],
  datasources: ["User-Guide/04-pipelines.md", 2],
  pipelines: ["User-Guide/04-pipelines.md", 3],
  endpoints: ["User-Guide/05-endpoints-and-sharing.md", 2],
  policies: ["User-Guide/05-endpoints-and-sharing.md", 6],
  ckan: ["User-Guide/09-export-import.md", 4],
  dashboards: ["User-Guide/06-dashboards.md", 2],
  explore: ["User-Guide/06-dashboards.md", 4],
  approvals: ["User-Guide/07-users-roles-approvals.md", 3],
  activity: ["User-Guide/07-users-roles-approvals.md", 3],
  assistant: ["User-Guide/08-working-with-ai-agents.md", 1],
  knowledge: ["User-Guide/08-working-with-ai-agents.md", 1],
  workspaces: ["User-Guide/02-organizations-projects-spaces.md", 5],
  apps: ["User-Guide/11-apps.md", 1],
  settings: ["User-Guide/12-managing-organization-and-projects.md", 2],
};

/** Markdown inline syntax reduced to its words: links to their text, emphasis and code marks dropped. */
export function plain(text) {
  return text
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/<[^>]+>/g, "")
    .replace(/(\*\*|__)(.+?)\1/g, "$2")
    .replace(/(^|[^\w*])[*_]([^*_\s][^*_]*?)[*_](?![\w*])/g, "$1$2")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * The blocks of H2 section `number` of a guide page: `["h", text]`, `["p", text]`, `["ul", items]`,
 * `["ol", items]` and `["pre", code]`. Images, tables, admonition fences and mermaid diagrams are
 * left out: the panel links the full page for them. A missing section throws.
 */
export function sectionBlocks(markdown, number) {
  const lines = markdown.split(/\r?\n/);
  const start = lines.findIndex((line) => new RegExp(`^## ${number}\\.\\s`).test(line));
  if (start < 0) throw new Error(`no section "## ${number}." in the page`);
  const heading = plain(lines[start].replace(/^## \d+\.\s*/, ""));
  const blocks = [];
  let para = [];
  let list = null;
  const flush = () => {
    if (para.length) blocks.push(["p", plain(para.join(" "))]);
    if (list) blocks.push(list);
    para = [];
    list = null;
  };
  for (let at = start + 1; at < lines.length; at++) {
    const line = lines[at];
    if (/^## /.test(line)) break;
    const fence = line.match(/^\s*(```|~~~)\s*(\S*)/);
    if (fence) {
      flush();
      const body = [];
      for (at++; at < lines.length && !lines[at].trim().startsWith(fence[1]); at++) body.push(lines[at]);
      if (fence[2] !== "mermaid" && body.some((l) => l.trim())) blocks.push(["pre", body.join("\n")]);
      continue;
    }
    const item = line.match(/^\s{0,3}([-*+]|\d+[.)])\s+(.*)$/);
    if (item) {
      const kind = /\d/.test(item[1]) ? "ol" : "ul";
      if (para.length || (list && list[0] !== kind)) flush();
      list ??= [kind, []];
      list[1].push(plain(item[2]));
      continue;
    }
    if (!line.trim() || /^\s*(:::|\||!\[)/.test(line)) {
      flush();
      continue;
    }
    if (/^#{3,6} /.test(line)) {
      flush();
      blocks.push(["h", plain(line.replace(/^#+\s*/, ""))]);
      continue;
    }
    if (list && /^\s+\S/.test(line)) {
      // A wrapped list item continues the last one.
      list[1][list[1].length - 1] = plain(`${list[1].at(-1)} ${line}`);
      continue;
    }
    if (list) flush();
    para.push(line);
  }
  flush();
  return { heading, blocks: blocks.filter((block) => block[1].length > 0) };
}

/** Every page's section, read from `read(path)`, under the commit they came from. */
export function bundle(commit, read) {
  if (!/^[0-9a-f]{40}$/.test(commit)) throw new Error(`pin a full docs commit sha, not ${JSON.stringify(commit)}`);
  const sections = {};
  for (const [key, [source, number]] of Object.entries(SECTIONS)) {
    sections[key] = { source, number, ...sectionBlocks(read(source), number) };
  }
  return { docsCommit: commit, sections };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [docs, ref] = process.argv.slice(2);
  if (!docs || !ref) {
    console.error("usage: node scripts/guide-sections.mjs <docs-clone> <commit>");
    process.exit(2);
  }
  const git = (...args) => execFileSync("git", ["-C", docs, ...args], { encoding: "utf8", maxBuffer: 16 << 20 });
  const commit = git("rev-parse", "--verify", `${ref}^{commit}`).trim();
  const out = join(dirname(fileURLToPath(import.meta.url)), "../ui/src/generated/guideSections.json");
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, `${JSON.stringify(bundle(commit, (path) => git("show", `${commit}:${path}`)), null, 1)}\n`);
  console.log(`${out}: ${Object.keys(SECTIONS).length} sections from docs ${commit}`);
}
