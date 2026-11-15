# CKAN

The open-data catalogue: the public face of every Endpoint a project chooses to publish
(EP-62 … EP-67). `jcctl` writes datasets and resources into it through the Action API, and
the Portal reads their status back; this component runs the CKAN those two talk to.

Deploy the component with:
```bash
helmfile apply -i --selector component=ckan
```

## What it deploys

`charts/ckan`, which is three workloads and two claims:

- **CKAN** itself, one replica over a ReadWriteOnce claim that holds uploads and resources.
  A second replica would not schedule beside the first, so scaling needs an RWX volume or
  object storage before it needs a replica count.
- **Solr**, from the image the CKAN project publishes with CKAN's own schema already
  loaded. A plain Solr cannot index a CKAN: the schema is the whole point of that image.
- **Redis**, cache and job queue, with persistence off. A restart costs a rebuild of what
  CKAN recomputes anyway.

PostgreSQL comes from CloudNativePG through `databases.yaml`, and the databases are two:
the catalogue's own and the DataStore's. CKAN refuses to run them in one database.

## The DataStore read role

`datastore_search_sql` runs SQL the caller wrote, on the read connection. That is safe only
while the read connection can do nothing but `SELECT`, which is why `databases.yaml`
declares `ckan_datastore_read` as an `additionalRoles` read-only role with its own
generated password, and the chart builds `CKAN_DATASTORE_READ_URL` from it. Pointing both
URLs at the owner turns a public catalogue into an arbitrary-SQL surface; CKAN checks this
at startup and refuses to start.

## What fills and refreshes the DataStore

The Portal's reconciler, on every sync pass of the replica that holds the reconciler lock
(`JC_PORTAL_SYNC_INTERVAL`, 60 s by default), publishes every Endpoint whose manifest declares
`publish.ckan` and, when it declares `publish.ckan.datastore`, re-reads the Endpoint's tabular
answer as an ordinary consumer and writes it into the dataset's sheet (portal
`src/reconciler/ckan.rs`, through `jcctl::publish::ckan_datastore`, EP-62, EP-65). There is no
separate job and no second schedule: the `jcctl publish ckan` run from a shell that EP-65 once
described is the same code, now driven by the reconciler.

So a sheet holds what the Endpoint answers at the last pass, and nothing more. A dataset whose
pipelines have not run, or were refused by a quota, publishes a near-empty sheet until they do;
the fix is upstream of the catalogue, never a fill by hand.

## Catalogue harvests

Some datasets have no Endpoint behind them: a city publishes files in the national catalogue
(NKOD) and the assistant has to answer from them (T-4151). `harvest.sources` lists one entry per
publisher, and the chart renders one CronJob per entry (`ckan-harvest-<name>`, nightly). Each run
of `files/harvest-nkod.py`:

- reads the publisher's datasets from the NKOD SPARQL endpoint, with the licence from
  `legislation:termsOfUse` (NKOD carries no `dct:license`), and leaves out a dataset with no open
  licence;
- fetches each distribution once over https, only from `allowedHosts`, redirects included, with
  TLS verification on. An HTML or PDF answer where JSON is declared is not ingested, a non-ASCII
  URL is percent-encoded, and string values are trimmed;
- skips the titles under `skip` (a link to another dataset's file, personal data) and drops the
  rows of a `requireColumn` table that have no value in that column. A filtered table is
  published without the original file, which still holds those rows;
- writes each dataset into the source's organization with its licence, publisher and `modified`,
  each file as a link and each flat JSON table as a DataStore table (CKAN also serves it as CSV),
  then withdraws what an earlier run harvested and the publisher no longer lists;
- checks every ingested dataset (title, licence, a format on each resource, the row count of each
  table), prints the count per state with each skipped title and its reason, and exits 1 when a
  check fails. An empty catalogue answer deletes nothing.

It writes with the publisher token above and mounts no Kubernetes token. Its egress is DNS, the
catalogue and public addresses on 443 with the private ranges cut out (`ckan-harvest`), so CKAN
itself still dials nothing. The first run after an apply does not wait for the schedule:

```bash
kubectl -n <ns> create job --from=cronjob/ckan-harvest-banskabystrica ckan-harvest-banskabystrica-now
kubectl -n <ns> logs -f job/ckan-harvest-banskabystrica-now -c harvest
```

### `kind: hub`: an ArcGIS Hub (T-4152)

An entry with `kind: hub` runs `files/harvest-hub.py` instead. On dev, BBSK's Hub
(`https://opendata.bbsk.sk`) fills `bbsk` (`ckan-harvest-bbsk`). Its egress and token are the
ones above.

| Rule | What the job does |
|---|---|
| list | reads the Hub search API whole (`numberMatched` items); a short read is an error |
| licence | keeps CC-BY-4.0, CC-BY-SA-4.0 and CC0-1.0; skips anything else, naming why |
| corrections | `skip` (by item id, with the reason), `titles` (by item id), `spelling` from the values |
| duplicates | one title, one dataset: the newest `modified` stays, the other is logged as its duplicate |
| resources | per layer a CSV and, with a geometry, a GeoJSON from the Hub's download API, plus the ArcGIS REST service; a link off the source's `allowedHosts` is dropped |
| identity | extras `harvest_source` and `source_id`; the CKAN id is a UUIDv5 of the item's Hub URL |
| withdrawal | after a complete read, a dataset of this source that the read no longer lists is deleted (CKAN keeps it as `deleted`, and a returning item revives it); nothing else in the organization is touched |

Its log names every skipped item and why.

## Branding

Nothing here carries a city name. `global.branding` is written into the theme ConfigMap as
`branding.json` and read by `ckanext_jc_theme` (OPS-46, OPS-47); the Portal is handed the same
block as YAML, so the two surfaces cannot drift. What a deployment sets, and where it shows:

```yaml
global:
  branding:
    instanceName: joinedcontext       # header, hero, footer, page titles
    organisation: City of ...         # footer, publisher fallback
    contactEmail: opendata@example.org
    logo: logo.svg                    # header and footer mark; logoSvg carries the bytes
    favicon: logo.svg                 # a file the ConfigMap carries (today: the logo)
    logoSvg: |
      <svg ...>
    colours: {primary: '#0000bf', secondary: '#0072c6', accent: '#ffe977', background: '#ffffff', text: '#1a1a1a'}
    fonts: {heading: 'system-ui, sans-serif', body: 'system-ui, sans-serif'}
    languages: {default: en, offered: [en]}
    tagline: ''                       # under the name on the home page; empty = the theme's line
    footerLines: []                   # footer paragraphs: a demo disclaimer, an imprint
```

The look is `files/jc_theme/public__jc-theme.css`, in the Portal's family (T-3009): a light header,
cards on a tinted page, the brand colour for actions and the current place, a dark footer.
`templates__base.html` writes the five colours and two font stacks as `--jc-*` custom properties
and the stylesheet mixes every other shade from them with `color-mix()`, the way the Portal's
`tokens.css` does, so it holds no colour of its own and a second city restyles the catalogue from
this block alone. CKAN's own teal, which its Bootstrap build writes into dozens of rules, is
restated on the brand there too. Pages the theme lays out itself: the home page (hero, search,
recently updated datasets, publishers and keywords), the dataset page (every DataStore table, one per
entity type, framed first with its row count, the first open, then About, the live API, and the resources in three sections: files,
APIs, and the folded data model), the resource rows, the search results and the footer.

`jc_theme` is the first plugin in `ckan.plugins`: the first plugin's templates win, and the theme
overrides the table view's own page (`datatables/datatables_view.html`), which empties the styles
block every other page inherits, so the grid framed on a dataset page wears the same look.

A rebrand is a values edit and an apply; the checksum annotation on the Deployment restarts the
pod so the change is seen.

Every page offers the knowledge assistant when the environment lists the `assistant`
component and sets `ckan.catalogue.assistant` to the publicId of a `ckan` AssistantDeployment
(AG-114, T-3058): a button in the corner opens the widget page of
`https://assistant.{domain}/d/{publicId}/widget` in a frame. That deployment's
`allowedOrigins` has to name this catalogue's host, or the assistant's own `frame-ancestors`
refuses the frame. The theme offers only an address on the installation's assistant host.

The theme is mounted, not baked: a ConfigMap has no directories, so each file is mounted by
its own `subPath` into one directory on `PYTHONPATH`, together with a `dist-info` carrying
the `entry_points.txt` that makes the plugin discoverable. A plugin CKAN cannot find is a
plugin CKAN ignores.

## Identity

A person signs in to the catalogue with their Keycloak account, through
[`ckanext-oidc-pkce`](https://github.com/DataShades/ckanext-oidc-pkce) and the confidential
`ckan` client. One flag turns the whole thing on: `global.ckan.sso` adds the plugin to
`ckan.plugins`, writes the realm's four endpoints into the ini, and opens the egress the login
needs. It ships **on**, against the image pinned below; a plugin CKAN cannot import stops CKAN
from starting at all, so the flag and that digest move together and a render test refuses a
render where they disagree.

The extension is not the `ckanext-oauth2` the original task named. That one calls `before_map()`
and reads `repoze.who.identity`, and CKAN 2.10 removed both; its last commit is from 2019, and
its master branch is still Python 2. `ckanext-oidc-pkce` is tested upstream against CKAN 2.9,
2.10 and 2.11, speaks plain OIDC — which is what the realm serves — and takes a client secret,
so the client stays confidential.

The stock `ckan/ckan-base` carries no OIDC authenticator, so `images/ckan/Dockerfile` adds one
and nothing else. `.github/workflows/image-ckan.yml` builds it, signs it with cosign and scans
it, the way every image this platform deploys is published (OPS-27); the digest it prints is what
`images.yaml` pins as `ckan.catalogue`. The image is also where the catalogue's patch level lives:
it is built from `ckan/ckan-base:2.11.7`, takes the day's Debian security updates, upgrades the
Python packages that have a published fix, and carries neither the kernel headers the base image
keeps for building wheels nor pip itself — 3380 scanner findings on the stock image, none on
this one, and nothing hidden behind an ignore rule.

The client id, the client secret and the realm's host reach the extension as environment
variables, which is how it reads those three (`os.environ.get` in its `config.py`) — so the
secret is a `secretKeyRef` and never a line in the ini. Everything it appends to that host it
reads through `tk.config`, and its defaults are Okta's paths, so `files/startup/01-oidc.sh`
writes the realm's authorize, token, userinfo and logout paths into `$CKAN_INI` before uWSGI
starts.

## The secret key, and why it is written after CKAN's own entrypoint

CKAN signs the session cookie, the CSRF token and every API token with one value:
`SECRET_KEY`. `WTF_CSRF_SECRET_KEY` and both `api_token.jwt.*.secret` options derive from it
when they are unset, and `beaker.session.secret` is its own legacy name — not a second secret.

`start_ckan.sh` mints that value itself when the ini has none, and the test it guards that
branch with reads the *relative* `ckan.ini`, which is the pristine one in the image and always
empty. The branch therefore fires on every start, whatever the copy on the `emptyDir` holds,
and because the `emptyDir` is new with every pod, a restart used to invalidate every session
and every issued API token with nothing in the log naming the cause (T-0436).

An env var cannot fix that, and the `CKAN___BEAKER__SESSION__SECRET` this component used to
set never did anything. **CKAN 2.11 has no `CKAN___<OPTION>` override.** `CKANConfigLoader`
copies every `CKAN_*` variable into the ConfigParser defaults under its own literal name, and a
value in `[app:main]` shadows a default — so the line the entrypoint has just written always
wins. Only `CONFIG_FROM_ENV_VARS` in `ckan/config/environment.py` maps env to options, and it
lists `CKAN_SITE_URL`, `CKAN_SQLALCHEMY_URL`, `CKAN_SOLR_URL` and a dozen more, none of them a
secret key.

So the value is written into the ini, from `files/startup/00-secret-key.sh` mounted at
`/docker-entrypoint.d`. The entrypoint sources that directory after its own minting branch and
after `prerun.py`, and before uWSGI, which is the one point in the image's startup where a
stable value wins without patching the image. The value comes from the generated `ckan-session`
Secret through `JC_CKAN_SECRET_KEY` — a name deliberately outside the `CKAN_` prefix, so it
cannot end up in the config as a key of its own. A pod that finds it empty exits rather than
serving with a key that dies with it.

The Secret is still named `ckan-session` because renaming it would generate a new one, and a
new key logs everyone out and invalidates every token they hold.

The image's ini also ships `SESSION_COOKIE_SECURE = false` and `REMEMBER_COOKIE_SECURE = false`.
`files/startup/02-secure-cookies.sh` writes both `true` whenever `CKAN_SITE_URL` is https, so the
`ckan` and `remember_token` cookies carry `Secure` (T-3017); a plain-http site URL keeps the
default, which a browser needs to send the cookie back at all.

## DCAT-AP for harvesters

With `global.ckan.dcat` on, the catalogue serves DCAT-AP 3 through `ckanext-dcat` (the image
built from `images/ckan/Dockerfile` carries it, T-3010):

- `/catalog.ttl` (also `.rdf`, `.jsonld`, `.xml`): the whole catalogue, 100 datasets per page,
  the next page named by `hydra:nextPage`.
- `/dataset/{name or id}.ttl` (same formats): one dataset with its distributions.

The profiles are `euro_dcat_ap_3` and then the theme's `jc_dcat_ap`
(`charts/ckan/files/jc_theme/dcat_profile.py`), which turns media types, formats, themes,
languages and checksum algorithms into the controlled-vocabulary IRIs the DCAT-AP 3.0.1
shapes ask for, gives every NGSI-LD, MCP, OGC API and SensorThings resource a
`dcat:DataService` naming its standard, and puts the licence on every distribution.

What a harvester reads comes from two places, and nothing in it is typed into the chart:

| DCAT-AP | Source |
|---|---|
| catalogue `dct:title`, `dct:publisher` (`foaf:name`, `foaf:mbox`), `dct:language` | `global.branding`: `instanceName`, `organisation`, `contactEmail`, `languages.offered` |
| dataset title, description, keywords, themes, publisher, spatial, licence | the Endpoint's catalogue record, as the publisher writes it into CKAN |
| dataset `dcat:contactPoint` when the record names none | `global.branding.contactEmail`, under `instanceName` |

**Harvesting us.** Another CKAN adds a harvest source of type **DCAT RDF Harvester**
(`ckanext-dcat`'s `dcat_rdf` harvester) with the URL `https://opendata.{domain}/catalog.ttl`; it
follows the pages itself. data.europa.eu and a national portal (data.gov.sk, data.gov.cz)
take the same URL as their DCAT-AP source. A re-harvest updates rather than duplicates
because each dataset keeps its URI (`https://opendata.{domain}/dataset/{CKAN id}`, which lives
as long as the dataset) and its `dct:identifier` (the Endpoint's slug, which never changes);
the publisher is the organization the Endpoint publishes under, so renaming it in CKAN
changes the name a harvester shows and nothing else.

**Checking it.** `scripts/check-dcat-ap.py https://opendata.{domain}` reads every catalogue page
and every dataset and validates each against the DCAT-AP 3.0.1 shapes in
`scripts/dcat-ap-3.0.1/` (from github.com/SEMICeu/DCAT-AP, `releases/3.0.1/shacl`); it
exits 1 on any violation and on an empty catalogue. `tests/test_ckan_dcat.py` runs the same
check in CI on a real record serialized inside the pinned image.

## Routing

The catalogue is published on `opendata.{domain}`, derived from the route's `subDomain`, which
also puts the host on the edge Ingress and in the certificate SAN list. Its old host
`data.{domain}` keeps its Ingress rule and certificate and answers only a 301 to the same path
and query on `opendata.{domain}` (T-4679), for links shared before the move. `/ckan` on the
primary host answers a 302 to that host rather than proxying: CKAN builds every link it
renders from `CKAN_SITE_URL`, so a proxied prefix would emit links off the prefix anyway.

## Content Security Policy

Every HTML page CKAN renders carries the theme's own policy (T-5147, `_csp` in `plugin.py`):
scripts and styles from this host or with the request's nonce, frames, fetches and images from
`{domain}` and its subdomains (the Endpoints, `tiles.`, `viz.`, `assistant.`), `form-action`
and `frame-ancestors 'self'`, no `'unsafe-inline'` or `'unsafe-eval'`. The map page `/map/{id}`
sends the same policy with `frame-ancestors *`. The edge's frame-ancestors-only baseline still
covers the answers CKAN sends without a policy (static files, JSON).

A template that writes an inline `<script>` or `<style>` gives it `nonce="{{ h.jc_csp_nonce() }}"`;
an inline handler or a `style` attribute is refused, so the theme overrides the CKAN 2.11.7
templates that carry one (`dataviewer/base.html`, `text_view.html`, `datatables_view.html`,
`resource_upload_field.html`, `image_view.html`) and `jc-forms.js` does what their `onclick`
did. A CKAN upgrade re-checks those copies against the new templates. Gravatar is off
(`CKAN__GRAVATAR_DEFAULT`): its images come from outside the policy and would send each
account's e-mail hash to gravatar.com.

## Running under the runtime policies

Kyverno is in Enforce mode on dev, so every container here runs non-root with a read-only
root filesystem. That costs three arrangements:

- `CKAN_INI` lives in an `emptyDir` at `/tmp`, because `start_ckan.sh` edits it in place
  with `ckan config-tool` at every start and has nowhere else to write. The entrypoint only
  edits that file, it never creates it, so a `seed-config` init container copies the
  `/srv/app/ckan.ini` the image was built with into the `emptyDir` first.
- Solr writes its logs beside its binaries by default; `SOLR_LOGS_DIR` moves them onto its
  claim.
- Redis persistence is off, so it needs no writable path at all.
