#!/usr/bin/env bash
# T-5129: the embed snippets pasted into a real WordPress page (a "Custom HTML" block, as a town's
# web editor pastes them), then wordpress.spec.mjs reads the pages at 1440 and 400. Starts
# WordPress and MariaDB in Docker, removes both (and their volume) at the end.
#   components/viz/tests/wordpress/run.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
port="${WP_PORT:-8089}"
export WP_PORT="$port"
compose=(docker compose -f "$here/compose.yaml")
trap '"${compose[@]}" --profile cli down -v >/dev/null 2>&1 || true; rm -rf "${here:?}/pages"' EXIT
mkdir -p "$here/pages"
# The two snippets exactly as the Embed dialog writes them (embed.js), for the routed test Endpoint.
node --input-type=module -e '
  import { createRequire } from "node:module";
  import { writeFileSync } from "node:fs";
  import { SLUGS, VIZ } from "'"$here"'/../explore-hosts.mjs";
  const e = createRequire(import.meta.url)("'"$here"'/../../charts/renderer/files/site/embed.js");
  const o = { slug: SLUGS.point, dataset: "bike-stations", type: "BikeHireDockingStation", lang: "sk", height: 760 };
  const block = (code) => "<!-- wp:paragraph --><p>Aktuálny stav staníc priamo z otvorených dát mesta.</p><!-- /wp:paragraph -->\n<!-- wp:html -->\n" + code + "\n<!-- /wp:html -->";
  writeFileSync("'"$here"'/pages/iframe.html", block(e.snippet("iframe", o, "https://" + VIZ, "Stanice bicyklov")));
  writeFileSync("'"$here"'/pages/component.html", block(e.snippet("component", { ...o, view: "table" }, "https://" + VIZ, "Stanice bicyklov")));
'
"${compose[@]}" up -d --wait wordpress
for _ in $(seq 60); do curl -fsS -o /dev/null "http://127.0.0.1:$port/wp-admin/install.php" && break; sleep 2; done
wp=("${compose[@]}" run --rm cli wp)
"${wp[@]}" core install --url="http://127.0.0.1:$port" --title="Mesto Banská Bystrica" --admin_user=editor \
  --admin_password="$(head -c 18 /dev/urandom | base64)" --admin_email=editor@banskabystrica.example --skip-email >/dev/null
"${wp[@]}" rewrite structure '/%postname%/' >/dev/null
"${wp[@]}" post create /pages/iframe.html --post_type=page --post_status=publish --post_title="Zdieľané bicykle" --post_name=bicykle --user=editor >/dev/null
"${wp[@]}" post create /pages/component.html --post_type=page --post_status=publish --post_title="Bicykle: tabuľka" --post_name=bicykle-tabulka --user=editor >/dev/null
cd "$here/../../../.."
WP_URL="http://127.0.0.1:$port" PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-/ms-playwright}" \
  npx playwright test -c components/viz/tests --reporter=line "$@"
