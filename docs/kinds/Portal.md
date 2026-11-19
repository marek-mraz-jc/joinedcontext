# Portal

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Portal`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/portals`
- Repository path: `portals/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (PortalSpec)

Desired specification of a `Portal` (ODP-08).

| Field | Type | Required | Description |
|---|---|---|---|
| `appearance` | [OrganizationAppearance](#organizationappearance) | no | Overrides of the Organization's appearance for this portal only. |
| `hidden` | list of string | no | Dataset names the portal does not list. |
| `host` | string | no | A host the operator listed for the Organization, e.g. `mobility.bratislava.sk`; absent: the slug address. |
| `languages` | list of string | no | The languages the portal is shown in, the first one the default, e.g. `[sk, en]`. |
| `organizationRef` | string | yes | The Organization the portal belongs to, e.g. `bratislava`. |
| `path` | string | yes | The portal's name inside the Organization, a DNS label such as `mobility`: `/{org}/opendata/-/mobility/`; `default` is the Organization's own portal. |
| `profiles` | list of string | no | The DCAT-AP profiles its feeds serve, e.g. `[dcat-ap-3.0.1]`. |
| `slug` | [SlugAddress](#slugaddress) | no | With a `host`: what the slug address does. |
| `sources` | list of [PortalSource](#portalsource) | no | Where its datasets come from (ODP-10). |
| `title` | map of string | no | The title on every page, per language, e.g. `{sk: Doprava, en: Mobility}`. |

## OrganizationAppearance

`spec.appearance` (UIX-16).

| Field | Type | Required | Description |
|---|---|---|---|
| `city` | string | no | The city or region the Organization serves, e.g. `Banská Bystrica`. |
| `colours` | [AppearanceColours](#appearancecolours) | no | The colour tokens, each a hex colour such as `#0b5394`. |
| `contactEmail` | string | no | The contact address in the footer and the open data portal, e.g. `opendata@banskabystrica.sk`. |
| `datasetList` | [DatasetListLayout](#datasetlistlayout) | no | How the open data portal lists datasets: `cards` (the default) or a compact `table`. |
| `faviconSvg` | string | no | The favicon as an SVG document of at most 64 KiB, under the same rules as the logo. |
| `font` | [AppearanceFont](#appearancefont) | no | The open data portal's typeface, one of a fixed set of system font stacks, so no font is fetched from another host: `system`, `humanist`, `serif` or `mono` (T-7410). |
| `footerLinks` | list of [AppearanceLink](#appearancelink) | no | Links the open data portal's footer adds after its own, under the same rules. |
| `hero` | [AppearanceHero](#appearancehero) | no | The open data portal's home hero: its title and an introduction in Markdown. |
| `homeWidgets` | list of string | no | Dashboards the Organization's Portal home page shows, each `project/name` of one of its projects, e.g. `doprava/prehlad`; a viewer sees only those they may read (T-6779). |
| `instanceName` | string | no | Full name on page titles and the login page, e.g. `Mesto Banská Bystrica`. |
| `licenseDefault` | string | no | The licence a new dataset starts with, e.g. `CC-BY-4.0`. |
| `logoSvg` | string | no | The logo as an SVG document of at most 64 KiB, without script, event handler or external reference. |
| `navLinks` | list of [AppearanceLink](#appearancelink) | no | Links the open data portal's header adds after its own, at most 8, each a label and an `https://` address or a path on the portal, e.g. `{label: Mesto, url: https://mesto.sk}`. |
| `opendataWidgets` | list of string | no | Dashboards the Organization's open data portal shows on its catalogue home, each `project/name` of one of its projects with `visibility: public`, e.g. `doprava/prehlad`; anonymous readers see them (T-6780). |
| `pages` | list of [AppearancePage](#appearancepage) | no | The texts of the open data portal's own pages, each one page in one language in Markdown (T-7031); a page or language left out shows the built-in text. |
| `publisher` | string | no | The legal publisher named in the footer and the open data portal, e.g. `Mesto Banská Bystrica`. |
| `radius` | integer (uint8) | no | The corner radius of cards, buttons and fields in pixels, 0 to 24, e.g. `8`. |
| `shortName` | string | no | Short name on sidebars and tabs, e.g. `BB`. |

## SlugAddress

How a portal on its own host answers its slug address `/{org}/opendata/-/{path}/` (ODP-08).

Type: one of `"redirect"`, `"keep"`

## PortalSource

One source of a portal (ODP-10). No source carries a credential in this version.

| Field | Type | Required | Description |
|---|---|---|---|
| `allowedHosts` | list of string | no | Every host a run may fetch from, the `url`'s own included. |
| `budget` | [HarvestBudget](#harvestbudget) | no | Records and bytes one run may read. |
| `convertedFile` | string | no | The deployment's file of converted datasets beside jc.toml, e.g. `od-datasets.json`: only those datasets are listed, with their Endpoint's distributions. |
| `datasetsBase` | string | no | The catalogue base the converted file's dataset URLs name, replaced by this portal's own, e.g. `https://opendata.dev.joinedcontext.com/`. |
| `enabled` | boolean | no | Whether the source runs on its schedule; `false` keeps it and its records without runs. |
| `gateway` | string | no | The public base the Endpoints of converted datasets answer under. |
| `name` | string | yes | Unique in the portal, a DNS label, e.g. `kosice`. |
| `namePrefix` | string | no | A prefix for each record's name, e.g. `ke-`. |
| `names` | list of string | no | `endpoints`: only these Endpoints by name. |
| `projects` | list of string | no | `endpoints`: only these projects' Endpoints. |
| `publisher` | string | no | The CKAN organisation the records land in, e.g. `kosice`. |
| `publisherDescription` | string | no | The publisher's description. |
| `publisherTitle` | string | no | The publisher's title, e.g. `Mesto Košice`. |
| `publisherUri` | string | no | The publisher's IRI, the one the records name. |
| `requireColumn` | map of string | no | A column each named dataset must have, dataset to column. |
| `rewriteHosts` | map of string | no | Hosts in harvested URLs replaced by others, e.g. `{old.sk: new.sk}`. |
| `schedule` | string | no | Five-field cron, UTC, e.g. `23 3 * * *`. |
| `skip` | map of string | no | Items left out (title, IRI or item id) with the reason the run log gives. |
| `spaces` | list of string | no | `endpoints`: only these spaces' Endpoints. |
| `spatialUri` | string | no | The IRI of the area the records cover. |
| `spelling` | map of string | no | Spelling fixed in titles and descriptions, wrong to right. |
| `tags` | list of string | no | `endpoints`: only Endpoints carrying one of these tags. |
| `titles` | map of string | no | Titles replaced, old title to new. |
| `type` | [PortalSourceType](#portalsourcetype) | yes | The source's type, e.g. `dcat-ap`. |
| `url` | string | no | A harvested catalogue's address, `https://` only. |

## AppearanceColours

`spec.appearance.colours`: each one a hex colour, `#rgb` or `#rrggbb`.

| Field | Type | Required | Description |
|---|---|---|---|
| `accent` | string | no | Highlight colour, e.g. `#f59e0b`. |
| `background` | string | no | Page background, e.g. `#ffffff`. |
| `primary` | string | no | Primary action colour, e.g. `#0b5394`. |
| `secondary` | string | no | Secondary colour for links and focus, e.g. `#0f766e`. |
| `text` | string | no | Body text, e.g. `#0f172a`. |

## DatasetListLayout

`spec.appearance.datasetList`.

Type: one of `"cards"`, `"table"`

## AppearanceFont

`spec.appearance.font`: a system font stack, never a web font (T-7410).

Type: one of `"system"`, `"humanist"`, `"serif"`, `"mono"`

## AppearanceLink

`spec.appearance.navLinks[]` and `footerLinks[]`.

| Field | Type | Required | Description |
|---|---|---|---|
| `label` | string | yes | The text of the link, e.g. `Mesto`. |
| `url` | string | yes | An `https://` or `http://` address or a path on the portal starting `/`, e.g. `https://www.banskabystrica.sk`. |

## AppearanceHero

`spec.appearance.hero`.

| Field | Type | Required | Description |
|---|---|---|---|
| `intro` | string | no | The introduction under it in Markdown of at most 2 KiB, e.g. `Dáta mesta na **jednom mieste**.`; raw HTML shows as text. |
| `title` | string | no | The home page's heading, one line, e.g. `Otvorené dáta Banskej Bystrice`. |

## AppearancePage

`spec.appearance.pages[]`: one page of the open data portal in one language (T-7031).

| Field | Type | Required | Description |
|---|---|---|---|
| `lang` | string | yes | The language as a two-letter code, e.g. `sk`. |
| `markdown` | string | yes | The page in Markdown, its first `# heading` the title, at most 16 KiB, e.g. `# Podpora\n\nNapíšte nám na opendata@mesto.sk.`; raw HTML shows as text. |
| `page` | string | yes | The page: `connector`, `privacy`, `terms`, `support` or `accessibility`. |

## HarvestBudget

The most a harvest run of a source reads before it fails and changes nothing (ODP-13).

| Field | Type | Required | Description |
|---|---|---|---|
| `bytes` | integer (uint64) | yes | Bytes read per run, e.g. `50000000`. |
| `records` | integer (uint64) | yes | Records per run, e.g. `5000`. |

## PortalSourceType

Where a [`PortalSource`] takes its datasets from (ODP-10).

Type: one of `"endpoints"`, `"dcat-ap"`, `"ckan-action"`, `"nkod-sparql"`, `"arcgis-hub"`
