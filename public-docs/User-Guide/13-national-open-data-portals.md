---
sidebar_position: 15
title: National Open Data Portals
description: How a publisher gets its public datasets listed on data.slovensko.sk or data.gov.cz, from the catalogue address to the one-time registration.
---

# National Open Data Portals

A national open data portal lists your datasets when you register your catalogue with it once; after that it reads the catalogue by itself and picks up every dataset you make public. This page is for the person who publishes a city's or a region's data: which address to give the portal, what has to be filled in first, and the one step on the portal's side that only you can take.

## 1. Before you register

Your project needs three things:

- **Publisher**, on **Project settings → General**, group *Its catalogue*: the name of your body and its identifier in the national register. A Slovak body uses `https://data.gov.sk/id/legal-subject/` followed by its IČO, for example `https://data.gov.sk/id/legal-subject/00313271` for the city of Banská Bystrica. A Czech body uses `https://rpp-opendata.egon.gov.cz/odrpp/zdroj/orgán-veřejné-moci/` followed by its IČO, for example `…/00064581` for Prague.
- **Contact point**, in the same group: the name of the desk that answers questions about the data and its shared e-mail address, never a person's own.
- **At least one public endpoint**: only endpoints whose audience is *Anyone* appear in the catalogue.

Each public endpoint also needs what the national portal checks, in the endpoint's **Catalogue description** section: a title and a description in the portal's language, themes, keywords in that language, how often the data is updated, the area it covers and a licence. A field left empty is left out of the record, and the portal reports it.

Your organization's administrator switches the national format on once for the whole organization: the organization's catalogue formats list `dcat-ap-sk` for the Slovak portal, `dcat-ap-cz` for the Czech one.

## 2. data.slovensko.sk

1. Take your catalogue's address: `https://api.{your installation}/catalog/{project}.ttl?profile=dcat-ap-sk`. For the city of Banská Bystrica on the development installation that is `https://api.dev.joinedcontext.com/catalog/banskabystrica.ttl?profile=dcat-ap-sk`.
2. Open it in a browser: you see your body as the publisher and one line per public endpoint.
3. Sign in to data.slovensko.sk as your body's publisher, open the list of local catalogues (*lokálne katalógy*), add a catalogue of the type *DCAT-AP Dokumenty* and paste the address.

The portal reads the catalogue on its own schedule. A dataset you make public later appears at its next reading; one you withdraw disappears from it.

## 3. data.gov.cz

1. Take your catalogue's address: `https://api.{your installation}/catalog/{project}.ttl?profile=dcat-ap-cz`, for Prague on the development installation `https://api.dev.joinedcontext.com/catalog/praha.ttl?profile=dcat-ap-cz`.
2. Open it in a browser and check the publisher and the datasets.
3. Fill in the registration form for local catalogues on data.gov.cz with that address, as the interface type *DCAT-AP Dokumenty*. Only a public body listed in the RPP register can register, so the form is signed by your body's data box.

## 4. When the portal reports an error

The portal names the dataset and the missing property. Open that endpoint's **Catalogue** tab, fill in what it names, and approve the change; the portal picks it up at its next reading. Nothing has to be registered again.

## Related

- [Endpoints and sharing](05-endpoints-and-sharing.md) — making an endpoint public and filling in its catalogue entry.
- [Managing your organization and projects](12-managing-organization-and-projects.md) — where the project's settings live.
