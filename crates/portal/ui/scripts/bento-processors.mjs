#!/usr/bin/env node
// The runner's processor catalog, read from the pinned image and never from a manual (PL-52):
//   docker run --rm ghcr.io/warpstreamlabs/bento:1.21.1 list --format json-full \
//     | node scripts/bento-processors.mjs src/schemas/bento-processors.json \
//         ../../joinedcontext-platform/crates/jc-core/src/kinds/bento_processors.rs
// Writes, per processor a pipeline step may name, its name, the runner's first category and its
// one-line summary. `command` and `subprocess` run programs inside the shared runner, `file`
// reads, writes and deletes the runner's files (its own summary says so) and `wasm` is the wasm
// compute kind (PL-34), so none of the four is a step; deprecated ones are left out. A project's
// runner holds every pipeline of that project in one process, so a step that reaches the host
// reaches every other pipeline's credentials and files (PL-07, PL-16, PL-50).
import { readFileSync, writeFileSync } from "node:fs";

import { helpText } from "./help-text.mjs";

const EXCLUDED = new Set(["command", "file", "subprocess", "wasm"]);

const [outPath, rustPath] = process.argv.slice(2);
if (!outPath) {
  console.error("usage: bento list --format json-full | node scripts/bento-processors.mjs <out.json> [<bento_processors.rs>]");
  process.exit(1);
}
const listing = JSON.parse(readFileSync(0, "utf8"));
const catalog = listing.processors
  .filter((processor) => processor.status !== "deprecated" && !EXCLUDED.has(processor.name))
  .map((processor) => ({
    name: processor.name,
    category: (processor.categories ?? [])[0] ?? "Utility",
    summary: helpText(processor.summary),
  }))
  .sort((a, b) => a.name.localeCompare(b.name));
writeFileSync(outPath, `${JSON.stringify(catalog, null, 2)}\n`);

if (rustPath) {
  const names = catalog.map((processor) => `    "${processor.name}",`).join("\n");
  writeFileSync(
    rustPath,
    `//! The processors a pipeline step may name (PL-52). Generated from \`bento list --format json-full\`
//! of ghcr.io/warpstreamlabs/bento:v1.21.1 by joinedcontext-portal/ui/scripts/bento-processors.mjs;
//! \`command\`, \`file\`, \`subprocess\` and \`wasm\` are left out on purpose. Change the pin and
//! rerun the script, never edit by hand.

/// Every processor name a \`processor:\` step accepts.
pub const PROCESSORS: &[&str] = &[
${names}
];
`,
  );
}
