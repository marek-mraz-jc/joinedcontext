---
sidebar_position: 24
title: Open Data Portals and Sources
description: How an Organization runs several open data portals, where each one answers, what a source is, how one portal federates with another DCAT-AP catalogue, and what each refusal means.
---

# Open Data Portals and Sources

An Organization publishes its open data through one or more **portals**. Each portal is a public catalogue with its own address, title, languages and look, and it lists the datasets of its **sources**: the Organization's own published Endpoints, and records harvested from other catalogues. This page is for the Organization administrator who sets portals up and for the data steward who decides what each one shows. The manifest fields are listed on the `Portal` kind page; the operations are in [API/16](../API/16-portals.md).

## 1. The Default Portal and Further Portals

Every Organization has the portal `default`. The platform creates it the first time it starts with this version, from the open data settings the Organization already had, and it keeps that catalogue: every dataset address and every dataset IRI stays what it was (ODP-09). `default` answers at `/{org}/opendata/`, the address the catalogue always had.

A further portal serves a theme or an audience of its own, for example `mobility` for transport data or `education` for the schools of a region (ODP-08). It answers at:

- `/{org}/opendata/-/{path}/` on the platform's shared host, for example `/bratislava/opendata/-/mobility/`. The `-` segment is never a dataset, group or page name, so a portal can never take over an address of `default`.
- or its own host, for example `mobility.bratislava.sk`, when the operator of the deployment has listed that host for your Organization. A host nobody listed is refused when the Change is proposed (section 4).

A portal on its own host chooses what its path address does with `spec.slug`. `redirect`, the default, answers every request on the path address `/{org}/opendata/[-/{path}/]...` with `308` to the same path and query on the host. `keep` answers on both, and every page and feed names the host as the canonical address.

Changing a portal's address never changes a dataset IRI: an IRI keeps the address the dataset was first published under, and that first address redirects to the new one (ODP-08).

A portal's `path` is a DNS label: lower-case letters, digits and hyphens, at most 63 characters, starting and ending with a letter or digit, and unique in the Organization. Example: `mobility`. Its manifest's `metadata.name` is the file's key among the manifests of every Organization, so it carries the Organization in front, for example `bratislava-mobility`; the `default` portal of an Organization needs no manifest until you describe it.

The deployment caps how many portals all its Organizations hold together: `portals.max` in the deployment configuration (`[dataroom] portals_max` in `jc.toml`), by default three per Organization the deployment serves. The operator lists the hosts an Organization's portals may take under that Organization's `hosts.portals`. Every `default` portal counts toward it and is always created.

## 2. Describe a Portal

A portal is configuration as code: one manifest in the Organization's repository, `portals/{name}.yaml`, and every change to it is a Change of that Organization that an administrator approves (ODP-08). The fields:

| Field | What it holds | Example |
|---|---|---|
| `metadata.name` | the manifest's key, unique in the deployment | `bratislava-mobility` |
| `spec.organizationRef` | the Organization the portal belongs to; it never changes | `bratislava` |
| `spec.path` | the portal's name in the Organization (section 1); `default` is the Organization's own portal | `mobility` |
| `spec.title` | the title on every page of the portal, per language | `{sk: "Doprava", en: "Mobility"}` |
| `spec.languages` | the languages the portal is shown in, the first one the default | `[sk, en]` |
| `spec.host` | its own host, one the operator listed for the Organization; absent means the path address | `mobility.bratislava.sk` |
| `spec.slug` | with a `host`: `redirect` (the default) answers the path address with `308` to the same page on the host; `keep` answers both and names the host in every link and identifier | `keep` |
| `spec.appearance` | overrides of the Organization's appearance (logo, colours) for this portal only | `{accent: "#0b6e4f"}` |
| `spec.profiles` | the DCAT-AP profiles the portal's feeds serve | `[dcat-ap-3.0.1, dcat-ap-sk]` |
| `spec.sources` | where its datasets come from (section 3) | |
| `spec.hidden` | dataset names this portal does not list | `[test-upload]` |

A second portal for a theme:

```yaml excerpt
apiVersion: joinedcontext.com/v1alpha1
kind: Portal
metadata:
  name: bratislava-mobility
  namespace: org
spec:
  organizationRef: bratislava
  path: mobility
  title:
    sk: Doprava v Bratislave
    en: Mobility in Bratislava
  languages: [sk, en]
  profiles: [dcat-ap-3.0.1]
```

## 3. Sources

A source tells the portal where its datasets come from (ODP-10). Each source has a `name`, unique in the portal, and a `type`:

- `endpoints`: this Organization's Endpoints that are already published in its `default` portal, narrowed by `projects`, `spaces`, `tags` (the keywords of the published dataset) or `names`; each list keeps the Endpoints that match one of its values, and the lists narrow together. A source never publishes anything by itself: an Endpoint appears only when it has opted in to publishing and carries a `public` Policy (see [Endpoints and Sharing](05-endpoints-and-sharing.md)). The portal shows the dataset under the `default` portal's IRI, and a dataset that leaves the selection leaves the portal within a minute. A project of another Organization is refused with `422` naming `spec.sources[i].projects`.
- `dcat-ap`, `ckan-action`, `nkod-sparql`, `arcgis-hub`: a catalogue elsewhere that the portal harvests on a `schedule`. Each takes the catalogue's `url` (`https://` only), `allowedHosts` (each inside the deployment's harvest allow-list), a `budget` of records and bytes per run, and the harvest options the deployment already knows: `publisher`, `publisherTitle`, `publisherUri`, `publisherDescription`, `spatialUri`, `namePrefix`, `rewriteHosts`, `skip`, `titles`, `spelling`, `requireColumn` and `gateway`.

Federation with another deployment, of this platform or of any DCAT-AP catalogue, is a `dcat-ap` source at that catalogue's public address:

```yaml excerpt
apiVersion: joinedcontext.com/v1alpha1
kind: Portal
metadata:
  name: bratislava-region
  namespace: org
spec:
  organizationRef: bratislava
  path: region
  title:
    en: Open data of the region
  languages: [en]
  sources:
    - name: kosice
      type: dcat-ap
      url: https://data.kosice.sk/kosice/opendata/catalog.ttl
      schedule: "0 3 * * *"
      allowedHosts: [data.kosice.sk]
      budget:
        records: 5000
        bytes: 50000000
```

A harvested record keeps its publisher, identifier, landing page and download links pointing at the source; the portal copies no data file, and marks the record as harvested with the source's name and its last run (ODP-13). Only `http` and `https` links are taken. A record the source withdraws disappears after the next complete run. A run that passes its budget stops with that reason and changes nothing. When two sources harvest one dataset, the first source in the list keeps it, matched by its identifier (ODP-14).

In this version no source carries credentials: a portal harvests only catalogues that answer without a login.

In the Portals area, a portal's **Sources** tab adds a source by type, each field with an example, and proposes it as a Change. **Test connection** reads the source's first page once, through the same guard as a run, and answers in one sentence: it answered as expected, it did not answer, the address is refused (not https, a host outside `allowedHosts` or the operator's allow-list, a private address, or a redirect to one), it answered in another format, or the answer is larger than the byte budget. Nothing of the remote answer is shown, and one person may test six times a minute, three at once. **Run now** starts a run; a second one while it is running is refused. **Last runs** is the run log: when each run started, how many records it created, changed, kept, withdrew and left out, and what stopped a failed run. The **Datasets** tab lists the portal's datasets with the source of each; **Hide** and **List again** propose a Change of `spec.hidden`, and a hidden dataset stays in the catalogue (ODP-15).

## 4. When a Change Is Refused

The Change names the field and the rule:

- **`name` is not a DNS label**: use lower-case letters, digits and hyphens, for example `mobility`.
- **A portal of this name exists**: names are unique in the Organization; choose another.
- **`host` is not listed for this Organization**: ask the deployment's operator to list the host, or leave `host` out to use the path address.
- **`portals.max (N) reached`**: the deployment holds N portals already. Delete a portal you no longer need, or ask the operator to raise the limit.
- **`default` cannot be deleted**: every Organization keeps its default portal; edit it instead.

## 5. Delete a Portal

Deleting a portal other than `default` removes its datasets, publishers and sources from the catalogue. Their addresses answer `410 Gone`, so a harvester or a search engine learns that the dataset left on purpose (ODP-09). The Endpoints themselves stay untouched in their projects.

## 6. Who Sees What

Each Organization's catalogues are kept apart in the database itself: a portal lists only its own Organization's Endpoints and what its own sources harvested (ODP-12). Another Organization's data enters only through a `dcat-ap` source at that Organization's public catalogue. An Organization administrator manages only the portals of their own Organization; the deployment's operators see every portal. Search, feeds, the CKAN API and the AI catalogue tools of a portal answer only for that portal.

The Portals area of the Portal, its own section beside the project pages, lists the portals, edits them and shows each source's runs (ODP-15).

## Related

- [The Catalogue and Public Pages](17-catalogue-and-public-pages.md)
- [National Open Data Portals](13-national-open-data-portals.md)
- [Endpoints and Sharing](05-endpoints-and-sharing.md)
- [API/16: Portals API](../API/16-portals.md)
