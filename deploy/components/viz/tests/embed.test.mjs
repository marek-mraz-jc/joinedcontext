// T-5129: the embed contract under node: the snippet generator, the /embed.html address it writes
// and the explorer reading that address back (query and order survive any character).
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const e = require("../charts/renderer/files/site/embed.js");
const x = require("../charts/renderer/files/site/explore.js");

const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const ORIGIN = "https://viz.dev.joinedcontext.com";

test("the iframe snippet frames /embed.html sandboxed, anonymous and sized", () => {
  const code = e.snippet("iframe", { slug: SLUG, dataset: "bike-stations", type: "BikeHireDockingStation", lang: "sk", height: 700 }, ORIGIN, "Stanice bicyklov");
  assert.equal(code, `<iframe src="${ORIGIN}/embed.html#${SLUG}&amp;dataset=bike-stations&amp;type=BikeHireDockingStation&amp;lang=sk" title="Stanice bicyklov" ` +
    'width="100%" height="700" style="border:0;max-width:100%" loading="lazy" referrerpolicy="no-referrer" ' +
    'sandbox="allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox"></iframe>');
  assert.doesNotMatch(code, /allow-top-navigation|allow-forms|Authorization|token/);
});

test("the web component snippet loads our script and carries the same settings as attributes", () => {
  const code = e.snippet("component", { slug: SLUG, dataset: "bike-stations", type: "Bike", view: "table", theme: "dark", q: 'name~="a \\"b\\" <c>"',
    sort: "availableBikeNumber;desc" }, ORIGIN, "Live data");
  assert.equal(code, `<script src="${ORIGIN}/embed.js" defer></script>\n<jc-data-explorer endpoint="${SLUG}" dataset="bike-stations" type="Bike" view="table" ` +
    'theme="dark" q="name~=&quot;a \\&quot;b\\&quot; &lt;c&gt;&quot;" sort="availableBikeNumber;desc" height="640" title="Live data"></jc-data-explorer>');
});

test("a title cannot break out of its attribute", () => {
  const code = e.snippet("iframe", { slug: SLUG }, ORIGIN, '"><script>alert(1)</script>');
  assert.match(code, /title="&quot;&gt;&lt;script&gt;alert\(1\)&lt;\/script&gt;"/);
  assert.doesNotMatch(code, /<script/);
});

test("wrong settings are refused with the field to fix", () => {
  const bad = [
    [{ slug: "../evil" }, /^endpoint/], [{ slug: SLUG, dataset: "A B" }, /^dataset/], [{ slug: SLUG, type: "a;b" }, /^type/],
    [{ slug: SLUG, lang: "fr" }, /^lang/], [{ slug: SLUG, view: "map" }, /^view/], [{ slug: SLUG, theme: "red" }, /^theme/],
    [{ slug: SLUG, height: 100 }, /^height/], [{ slug: SLUG, height: 2001 }, /^height/], [{ slug: SLUG, height: "64O" }, /^height/],
    [{ slug: SLUG, height: 640.5 }, /^height/], [{ slug: SLUG, q: "x".repeat(2001) }, /^q/],
    [{ slug: SLUG, sort: "a;b" }, /^sort/], [{ slug: SLUG, sort: "na me" }, /^sort/], [{ slug: SLUG, sort: "a;desc&x=1" }, /^sort/]
  ];
  for (const [o, why] of bad) assert.throws(() => e.hash(o), { message: why }, JSON.stringify(o));
  assert.throws(() => e.snippet("iframe", { slug: SLUG }, "https://evil.example"), /^Error: origin/);
  assert.throws(() => e.snippet("iframe", { slug: SLUG }, "javascript:alert(1)//viz.x"), /^Error: origin/);
});

test("the explorer reads back the query and order the snippet wrote, whatever they hold", () => {
  const q = 'status=="a&b,c#d=e";name~="100% \\"x\\" %20"';
  const h = e.hash({ slug: SLUG, dataset: "bike_stations-1", type: "Bike", lang: "en", view: "table", theme: "light", q, sort: "availableBikeNumber;desc" });
  const back = x.parseHash(h);
  assert.equal(back.slug, SLUG);
  assert.equal(back.dataset, "bike_stations-1");
  assert.equal(back.view, "table");
  assert.equal(back.q, q);
  assert.equal(back.sort, "availableBikeNumber;desc");
  // The page writes the same address back when the view moves (hashOf), dataset and view kept.
  assert.deepEqual(x.parseHash(x.hashOf(back)), back);
});

test("a hand-made address drops a wrong view, dataset or broken encoding", () => {
  const odd = x.parseHash(`#${SLUG}&q=%E0%A4%A&view=map&dataset=Bad Name&sort=a;b`);
  assert.equal(odd.q, null);
  assert.equal(odd.view, null);
  assert.equal(odd.dataset, null);
  assert.equal(odd.sort, null);
});
