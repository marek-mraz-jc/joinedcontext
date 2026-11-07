// Reconciles Administration → Costs with the cloud invoice (T-5126): the month's CSV from
// `GET /api/v1/organization/costs/export` against the invoice's net total, both in the price
// table's currency. AI lines are left out: the model provider bills them, not the cloud.
//
//   node scripts/cost-reconcile.mjs costs-2026-09.csv 31.40      # exits 1 past 10 %
//
// A gap means the price table (portal.costPrices in the deployment values) no longer matches
// what the cloud bills: a node, a volume or an address added, removed or repriced.
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** The share the two may differ by. */
export const TOLERANCE = 0.1;

/** The infrastructure lines of the export: everything but `ai`, in the CSV's own currency. */
export function infrastructure(csv) {
  const [header, ...lines] = csv.trim().split(/\r?\n/);
  const columns = header.split(",");
  const part = columns.indexOf("part");
  const cost = columns.indexOf("cost");
  const currency = columns.indexOf("currency");
  if (part < 0 || cost < 0 || currency < 0) throw new Error("no costs export: the header lacks part, cost or currency");
  let total = 0;
  const currencies = new Set();
  for (const line of lines) {
    // Only the item (an assistant's or a space's name) can be quoted: month, project and part come
    // before it as plain cells, and the amount and the currency after it, counted from the end.
    const cells = line.split(",");
    const at = (index) => cells[cells.length - (columns.length - index)];
    currencies.add(at(currency));
    if (cells[part] === "ai") continue;
    const value = Number(at(cost));
    if (!Number.isFinite(value)) throw new Error(`no amount in the line: ${line}`);
    total += value;
  }
  if (currencies.size > 1) throw new Error(`the export mixes currencies: ${[...currencies].join(", ")}`);
  return { total: Math.round(total * 100) / 100, currency: [...currencies][0] ?? "" };
}

/** Whether `page` is within the tolerance of `invoice`, and by how much it differs. */
export function reconcile(page, invoice) {
  if (!(invoice > 0)) throw new Error("the invoice total is a positive amount, e.g. 31.40");
  const gap = Math.abs(page - invoice) / invoice;
  return { ok: gap <= TOLERANCE, gap };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [file, amount] = process.argv.slice(2);
  if (!file || !amount) {
    console.error("usage: node scripts/cost-reconcile.mjs <costs-YYYY-MM.csv> <invoice net total>");
    process.exit(2);
  }
  const page = infrastructure(readFileSync(file, "utf8"));
  const { ok, gap } = reconcile(page.total, Number(amount));
  const percent = (gap * 100).toFixed(1);
  console.log(`page ${page.total.toFixed(2)} ${page.currency}, invoice ${Number(amount).toFixed(2)}: ${percent} % apart`);
  if (!ok) {
    console.error(`more than ${TOLERANCE * 100} % apart: update portal.costPrices to what the invoice bills`);
    process.exit(1);
  }
}
