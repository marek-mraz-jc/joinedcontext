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

- `/{org}/opendata/-/{name}/` on the platform's shared host, for example `/bratislava/opendata/-/mobility/`. The `-` segment is never a dataset, group or page name, so a portal can never take over an address of `default`.
- or its own host, for example `mobility.bratislava.sk`, when the operator of the deployment has listed that host for your Organization. A host nobody listed is refused when the Change is proposed (section 4).

A portal's `name` is a DNS label: lower-case letters, digits and hyphens, at most 63 characters, starting and ending with a letter or digit, and unique in the Organization. Example: `mobility`.

The deployment caps how many portals all its Organizations hold together: `portals.max` in the deployment configuration, by default three per Organization the deployment is sized for. Every `default` portal counts toward it and is always created.

## 2. Describe a Portal

A portal is configuration as code: one manifest in the Organization's repository, `portals/{name}.yaml`, and every change to it is a Change of that Organization that an administrator approves (ODP-08). The fields:

| Field | What it holds | Example |
|---|---|---|
| `metadata.name` | the portal's name (section 1) | `mobility` |
| `spec.title` | the title on every page of the portal, per language | `{sk: "Doprava", en: "Mobility"}` |
| `spec.languages` | the languages the portal is shown in, the first one the default | `[sk, en]` |
| `spec.host` | its own host, one the operator listed for the Organization; absent means the path address | `mobility.bratislava.sk` |
| `spec.appearance` | overrides of the Organization's appearance (logo, colours) for this portal only | `{accent: "#0b6e4f"}` |
| `spec.profiles` | the DCAT-AP profiles the portal's feeds serve | `[dcat-ap-3.0.1, dcat-ap-sk]` |
| `spec.sources` | where its datasets come from (section 3) | |
| `spec.hidden` | dataset names this portal does not list | `[test-upload]` |

A second portal for a theme:

```yaml
apiVersion: joinedcontext.com/v1alpha1
kind: Portal
metadata:
  name: mobility
spec:
  title:
    sk: Doprava v Bratislave
    en: Mobility in Bratislava
  languages: [sk, en]
  profiles: [dcat-ap-3.0.1]
  sources:
    - name: transport-endpoints
      type: endpoints
      projects: [transport]
```

## 3. Sources

A source tells the portal where its datasets come from (ODP-10). Each source has a `name`, unique in the portal, and a `type`:

- `endpoints`: this Organization's Endpoints that are already published, narrowed by `projects`, `spaces`, `tags` or `names`. A source never publishes anything by itself: an Endpoint appears only when it has opted in to publishing and carries a `public` Policy, or is listed as `public` (see [Endpoints and Sharing](05-endpoints-and-sharing.md)).
- `dcat-ap`, `ckan-action`, `nkod-sparql`, `arcgis-hub`: a catalogue elsewhere that the portal harvests on a `schedule`. Each takes the catalogue's `url`, `allowedHosts` (each inside the deployment's harvest allow-list), a `budget` of records and bytes per run, and the harvest options the deployment already knows (publisher, rewrite_hosts, skip, titles, spelling, require_column, converted_file, gateway, name_prefix).

Federation with another deployment, of this platform or of any DCAT-AP catalogue, is a `dcat-ap` source at that catalogue's public address:

```yaml
apiVersion: joinedcontext.com/v1alpha1
kind: Portal
metadata:
  name: region
spec:
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
