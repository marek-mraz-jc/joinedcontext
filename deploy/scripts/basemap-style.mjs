// T-4638: the basemap's styles, one per language, from the Protomaps style package, with every
// URL on the tile host. The chart fills __HOST__ and __FILE__ (components/basemap).
//   S=$(mktemp -d) && npm i --prefix "$S" --no-save @protomaps/basemaps@5.7.2 \
//     && NODE_PATH="$S/node_modules" node scripts/basemap-style.mjs components/basemap/charts/tiles/files/styles
import { createRequire } from "node:module";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

const { layers, namedFlavor } = createRequire(join(process.env.NODE_PATH ?? ".", "x"))("@protomaps/basemaps");
const out = process.argv[2];
if (!out) throw new Error("usage: node scripts/basemap-style.mjs <directory>");
for (const lang of ["sk", "en", "cs", "fi"]) {
  const style = {
    version: 8,
    name: `joinedcontext light (${lang})`,
    glyphs: "https://__HOST__/fonts/{fontstack}/{range}.pbf",
    sprite: "https://__HOST__/sprites/v4/light",
    sources: {
      protomaps: {
        type: "vector",
        url: "pmtiles://https://__HOST__/__FILE__",
        attribution: '<a href="https://protomaps.com">Protomaps</a> © <a href="https://openstreetmap.org/copyright">OpenStreetMap contributors</a>',
      },
    },
    // Each label in this language where OpenStreetMap has it, else the place's own name.
    layers: layers("protomaps", namedFlavor("light"), { lang }),
  };
  writeFileSync(join(out, `light-${lang}.json`), JSON.stringify(style) + "\n");
}
