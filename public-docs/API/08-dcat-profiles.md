---
sidebar_position: 9
title: National DCAT-AP Profiles
description: What DCAT-AP-SK 3.0.0 and DCAT-AP-CZ 2026-09-23 require of an Endpoint's catalogue record and of a publisher's catalogue beyond DCAT-AP 3.0, each rule with its source, the per-publisher catalogue a national portal harvests, and how the conformance suite checks it.
---

# National DCAT-AP Profiles

Every Endpoint serves a DCAT-AP 3.0 record ([EP-27](../Requirements/endpoints.md)), and the open data portal publishes it to CKAN. A national catalogue asks for more: the Slovak catalogue data.slovensko.sk harvests records that meet DCAT-AP-SK, the Czech NKOD records that meet DCAT-AP-CZ. This page lists what each profile adds to DCAT-AP 3.0, with the official text for every row. A rule without a source is not on this page. T-4499 writes the records that meet these rules; until then the conformance suite reports what each record lacks.

## 1. Profiles and versions

| Profile | Version pinned | Based on | Official text | Machine-readable shapes |
|---|---|---|---|---|
| DCAT-AP | 3.0.0 | DCAT 3 | <https://semiceu.github.io/DCAT-AP/releases/3.0.0/> | SEMIC SHACL, vendored |
| DCAT-AP-SK | 3.0.0 | DCAT-AP 3.0.0 | "Rozhranie katalógu otvorených dát DCAT-AP-SK 3", repository `slovak-egov/centralny-model-udajov`, folder `tbox/national/dcat-ap-sk/` at `15b09176edacf3fd2ba21612c1b8d950484fc076` (`index.bs`) | `povinné-vlastnosti/dcat-ap-sk-3.0.0-shapes.ttl` and `dcat-ap-sk-3.0.0-hvd-shapes.ttl` of the same folder, vendored |
| DCAT-AP-CZ | 2026-09-23 | DCAT-AP 3.0.0 | <https://ofn.gov.cz/dcat-ap-cz-otevřená-data/2026-09-23/> (open data), <https://ofn.gov.cz/dcat-ap-cz-hvd/2026-09-23/> (high-value datasets), <https://ofn.gov.cz/dcat-ap-cz-rozhraní-katalogů-dat/2026-09-23/> (catalogue interface) | OFN `shacl.ttl` per specification, vendored; plus our `cz-additions.shacl.ttl` for the cardinalities |

**Which Slovak version.** DCAT-AP-SK 3.0.0. The repository `datova-kancelaria/dcat-ap-sk-2.0` that published 2.0.1 is closed and points to `slovak-egov/centralny-model-udajov`, whose `tbox/national/dcat-ap-sk/README.md` names the standard "DCAT-AP-SK 3" on DCAT 3 and DCAT-AP 3.0.0, and whose `povinné-vlastnosti/` folder is the machine-readable form of its mandatory properties. Its text is written for a local catalogue (LKOD) that the national catalogue (NKOD, data.slovensko.sk) harvests. SK and CZ now share one base, DCAT-AP 3.0.0. The store of data.slovensko.sk (`https://data.slovensko.sk/api/sparql`, queried 2026-10-17) still holds 13,646 resources declaring DCAT-AP 2.1.1 beside DCAT-AP 3 terms; a 3.0 record is what the current text asks for, and the store already carries its terms.

**One defect in the published shapes.** `sha-dcatapsk:catalog_dataset` gives `dcat:dataset` of a catalogue `sh:maxCount 1`, so every catalogue with two datasets fails it, the standard's own example `príklady/katalóg.ttl` (three datasets) included. The suite reports that one result apart, as the upstream defect, and holds every other result as a violation; the vendored file stays byte for byte the upstream one.

**Which Czech version.** The OFN of 2026-09-23 is the newest. It replaced the single "Rozhraní katalogů otevřených dat" of 2024-05-28 with four specifications (catalogue interface, open data, HVD, data interfaces). Its published `shacl.ttl` files check value classes and datatypes but carry no `sh:minCount` except `hvdCategory`, so the mandatory rows of its tables are written as SHACL in `cz-additions.shacl.ttl`, each shape linking its row.

## 2. DCAT-AP-SK 3.0.0: what it adds

Each row is a property shape of `povinné-vlastnosti/dcat-ap-sk-3.0.0-shapes.ttl` (prefix `sha-dcatapsk:`), stricter than DCAT-AP 3.0. The section column is the anchor in `index.bs`.

| Class | Property | Rule | Shape | Section |
|---|---|---|---|---|
| Catalog | `dct:title`, `dct:description` | `rdf:langString`, one per language, at least one `@sk` | `catalog_title(_sk)`, `catalog_description(_sk)` | `#katalóg-názov`, `#katalóg-popis` |
| Catalog | `dct:publisher` | exactly one IRI matching `^https://data.gov.sk/id/legal-subject/` (the body's IČO) | `catalog_publisher` | `#katalóg-poskytovateľ` |
| Catalog | `dcat:contactPoint` | at most one, `vcard:Organization` or `vcard:Individual` | `catalog_contact` | `#katalóg-kontaktný-bod` |
| Catalog | `dcat:dataset` | IRIs of `dcat:Dataset` (the `maxCount 1` is the upstream defect of section 1) | `catalog_dataset(_type)` | `#katalóg-dataset` |
| Dataset | `dct:title`, `dct:description` | `rdf:langString`, one per language, at least one `@sk` | `dataset_title(_sk)`, `dataset_description(_sk)` | `#dataset-názov`, `#dataset-popis` |
| Dataset | `dct:publisher` | exactly one IRI matching `^https://data.gov.sk/id/legal-subject/` | `dataset_publisher` | `#dataset-poskytovateľ` |
| Dataset | `dct:issued`, `dct:modified` | at most one each, `xsd:dateTime` | `dataset_issued`, `dataset_modified` | `#dataset-dátum-publikácie`, `#dataset-dátum-modifikácie` |
| Dataset | `dcat:theme` | one or more IRIs (EU data-theme table) | `dataset_theme` | `#dataset-téma` |
| Dataset | `dct:accrualPeriodicity` | exactly one IRI (EU frequency table) | `dataset_frequency` | `#dataset-periodicita-aktualizácie` |
| Dataset | `dcat:keyword` | `rdf:langString`, at least one `@sk` | `dataset_keyword(_sk)` | `#dataset-kľúčové-slová` |
| Dataset | `dct:spatial` | one or more IRIs; NUTS-SK (`https://data.gov.sk/id/nuts1/SK0`) named first, EU tables accepted | `dataset_spatial` | `#dataset-register-adries`, `#dataset-geografické-územia` |
| Dataset | `dcat:contactPoint` | at most one, `vcard:Organization` or `vcard:Individual` | `dataset_contact` | `#dataset-kontaktní-bod` |
| Dataset | `dcat:distribution` | one or more IRIs of `dcat:Distribution` | `dataset_distribution` | `#dataset-distribúcia` |
| Dataset (HVD) | `dct:type`, `r5r:hvdCategory`, `r5r:applicableLegislation` | all three or none: type `…/dataset-type/HVD`, a category, ELI `http://data.europa.eu/eli/reg_impl/2023/138/oj` | `dataset_type_hvd`, `dataset_hvdCategory(_none)`, `dataset_applicableLegislation_hvd` | `#dataset-kategória-hvd` |
| Distribution | `leg:termsOfUse` | exactly one `leg:TermsOfUse` | `distribution_termsOfUse` | `#špecifikácia-podmienok-použitia` |
| Distribution | `dct:title` | `rdf:langString`, one per language | `distribution_title` | `#distribúcia-názov` |
| Distribution | `dct:conformsTo` | at most one IRI | `distribution_conformsTo` | `#distribuce-schéma` |
| Distribution (file) | `dcat:downloadURL`, `dct:format`, `dcat:mediaType` | exactly one IRI each; `dcat:accessURL` equal to `dcat:downloadURL`; no `dcat:accessService` | `distribution_downloadURL`, `_format`, `_mediaType`, `_accessURL_download`, `_accessService_none` | `#distribúcia-stiahnutie-súboru` to `#distribúcia-media-type` |
| Distribution (service) | `dcat:accessService` | one or more IRIs of `dcat:DataService` whose `dcat:endpointURL` equals the distribution's `dcat:accessURL`; no `dcat:downloadURL` | `distribution_accessService`, `_accessURL_service`, `_downloadURL_none` | `#distribúcia-služba` |
| DataService | `dct:title` | `rdf:langString`, at least one `@sk` | `dataService_title(_sk)` | `#dátová-služba-názov` |
| DataService | `dcat:endpointURL` | one or more IRIs | `dataService_endpointURL` | `#dátová-služba-prístupový-bod` |
| TermsOfUse | `leg:authorsWorkType`, `leg:originalDatabaseType`, `leg:databaseProtectedBySpecialRightsType`, `leg:personalDataContainmentType` | exactly one IRI each | `termsOfUse_*` | `#špecifikácia-podmienok-použitia` |
| Contact | `vcard:fn`, `vcard:hasEmail` | `fn` a `rdf:langString` with one `@sk`; one `mailto:` IRI | `contact_fn(_sk)`, `contact_hasEmail` | `#dataset-kontaktní-bod` |

`leg:` is `https://data.gov.sk/def/ontology/legislation/`, `r5r:` is `http://data.europa.eu/r5r/`. Three rules come from the IRI form of the harvest interface, not from a shape: datasets, distributions and data services are IRIs, never blank nodes (`#rozhranie-dcat-ap-dokumenty`).

## 3. DCAT-AP-CZ 2026-09-23: what it adds

Each row has cardinality `[1..1]` or `[1..*]` in the Czech tables. The source column is the row's own IRI, which resolves to the row in the specification.

| Class | Property | Cardinality | Value and vocabulary | Source |
|---|---|---|---|---|
| Dataset | `dct:title`, `dct:description` | [1..1] each | `rdf:langString` with a language tag | `https://ofn.gov.cz/dcat-ap-cz-otevřená-data#DatováSada.název`, `#DatováSada.popis` |
| Dataset | `dcat:keyword` | [1..*] | `rdf:langString` | `…#DatováSada.klíčovéSlovo` |
| Dataset | `dcat:theme` | [1..*] | EU data-theme table | `…#DatováSada.téma` |
| Dataset | `dct:accrualPeriodicity` | [1..1] | EU frequency table | `…#DatováSada.periodicitaAktualizace` |
| Dataset | `dct:publisher` | [1..1] | the publisher's IRI | `…#DatováSada.poskytovatel` |
| Dataset | `dct:spatial` | [1..*] | an element of RÚIAN (`https://linked.cuzk.cz/resource/ruian/`), or EU continent and country tables | `…#DatováSada.souvisejícíGeografickéÚzemí-PrvekZRúian` |
| Dataset, Distribution, DataService | `r5r:applicableLegislation` | [1..*] | ELI of the act; for open data `https://www.e-sbirka.cz/eli/cz/sb/1999/106/2025-08-19` | `…#DatováSada.příslušnýPrávníPředpis`, `#Distribuce.příslušnýPrávníPředpis`, `#DatováSlužba.příslušnýPrávníPředpis` |
| Distribution | `dcat:accessURL` | [1..1] | URL | `…#Distribuce.přístupovéUrl` |
| Distribution | `pu:specifikace` | [1..1] | a terms-of-use specification, below | `…#Distribuce.specifikacePodmínekUžití` |
| Distribution (file) | `dcat:downloadURL`, `dcat:mediaType`, `dct:format` | [1..1] each | IANA media type; EU file-type table | `…#Distribuce-SouborKeStažení.odkazNaStaženíSouboru`, `#Distribuce-SouborKeStažení.typMédia`, `#Distribuce-SouborKeStažení.formátSouboruKeStažení` |
| Distribution (service) | `dcat:accessService` | [1..1] | a DataService | `…#Distribuce-DatováSlužba.přístupováSlužba` |
| DataService | `dcat:endpointURL`, `dcat:endpointDescription`, `dct:title` | [1..1] each | URLs; text | `…#DatováSlužba.přístupovýBodSlužby`, `#DatováSlužba.popisPřístupovéhoBoduSlužby`, `#DatováSlužba.název` |
| Specifikace | `pu:autorské-dílo`, `pu:databáze-jako-autorské-dílo`, `pu:databáze-chráněná-zvláštními-právy`, `pu:osobní-údaje` | [1..1] each | licence or statement IRIs of the terms-of-use vocabulary | `…#SpecifikacePodmínekUžití.autorskéDílo`, `.originálníDatabáze`, `.zvláštníPrávoPořizovateleDatabáze`, `.osobníÚdaje` |
| Dataset, DataService (HVD) | `r5r:hvdCategory` | [1..*] | EU HVD category table | `https://ofn.gov.cz/dcat-ap-cz-hvd#DatováSada.kategorieHvd`, `#DatováSlužba.kategorieHvd` |
| DataService (HVD) | `dcat:contactPoint` | [1..1] | a `vcard:Kind` | `https://ofn.gov.cz/dcat-ap-cz-hvd#DatováSlužba.kontaktníBod` |
| Catalog | `dct:title`, `dct:description`, `dct:publisher`, `dcat:contactPoint`, `dcat:endpointURL` | [1..1] each | contact point a `vcard:Organization` with `vcard:fn` and `vcard:hasEmail` | `https://ofn.gov.cz/dcat-ap-cz-rozhraní-katalogů-dat#Katalog.název` to `#Katalog.přístupovýBodSlužby`, `#Organizace.email`, `#Organizace.jméno` |

`pu:` is `https://data.gov.cz/slovník/podmínky-užití/`, `r5r:` is `http://data.europa.eu/r5r/`. The HVD rows apply to a dataset listed under Implementing Regulation (EU) 2023/138; the catalogue rows to the catalogue a city's open data portal publishes, not to one Endpoint's record. A dataset series repeats the dataset rows (`…#SérieDatovýchSad.*`).

## 4. How the suite checks a record

The conformance repository vendors the shapes under `dcat/shapes/<profile>/<version>/`, byte for byte, each pinned by sha256 in `tests/ckan/profiles.py`; its README lists the source URL, version, digest and licence of every file. Updating a shape is a reviewed commit, never a fetch at test time. `tests/ckan/test_profiles.py` then:

- fails when a vendored file is not its pinned digest;
- holds each fixture record (`tests/ckan/fixtures/catalog-*.jsonld`, the gateway's records for a public, an organization and a restricted Endpoint) to DCAT-AP 3.0.0: a violation fails;
- holds each publisher catalogue fixture (`tests/ckan/fixtures/publisher-*.jsonld`, section 6) to its national profile: the Banská Bystrica catalogue in `?profile=dcat-ap-sk` to DCAT-AP-SK 3.0.0, the Praha one in `?profile=dcat-ap-cz` to DCAT-AP-CZ; a violation fails, except the upstream `catalog_dataset` defect of section 1, which the report lists apart;
- reports every DCAT-AP-SK and DCAT-AP-CZ violation of the single records into `evidence/dcat-profiles.md` and fails when that committed report is not a fresh run.

A third national profile is one more folder of shapes and one more entry in `PROFILES`; nothing else changes.

## 5. Choosing a profile

An Organization names the national profiles its records are served in, in `Organization.spec.dcatProfiles`: a list of `dcat-ap-sk` and `dcat-ap-cz`, empty by default. DCAT-AP 3.0 is always served and stays the default representation.

- **One rendering per profile.** The record of an Endpoint, the installation's catalogue and a publisher's catalogue take `?profile=<id>`: `dcat-ap` (the default), or a profile the Organization lists. A profile it does not list, or one this build was compiled without, answers 404. Every profile renders the same core record (title, description, publisher, themes, licence, distributions) and only adds or retypes what section 2 or 3 asks for, so the core values never differ between profiles.
- **Both profiles keep the checksums.** DCAT-AP-SK 3.0.0 and DCAT-AP-CZ stand on DCAT-AP 3.0.0, which takes SHA-256 in `spdx:checksum` and `xsd:nonNegativeInteger` in `dcat:byteSize`, so neither rendering drops or retypes them (EP-68).
- **SK and CZ stay two renderings.** Each national catalogue reads its own terms-of-use node and the Slovak one also splits the distributions (below), so one rendering per profile keeps each record to what its harvester reads.
- **What the Slovak rendering retypes.** A distribution that is a file (a `file.*` representation and every schema artifact) carries `dcat:downloadURL` equal to its `dcat:accessURL`, its `dct:format` from the EU file-type table by its media type, and no `dcat:accessService`. A distribution that is an interface (NGSI-LD, MCP, OGC API - Features, SensorThings, OData, WebSocket) gets a `dcat:DataService` of its own, `{accessURL}#service`, whose `dcat:endpointURL` is the distribution's access URL, beside the Endpoint's own service. A title or a contact name the gateway writes as one plain string, a representation's or an artifact's name and the desk's name, is written as `@sk`, the language the Slovak shapes require; an Endpoint's own titles keep their languages. A contact point is typed `vcard:Organization`. The original datasets of `dct:source` stay as IRIs without the `dcat:Dataset` type, so the national catalogue never harvests another publisher's dataset as this publisher's; the Czech rendering leaves them untyped too (T-4612).
- **Inputs.** The national terms-of-use node (`leg:TermsOfUse`, `pu:Specifikace`) is derived from `spec.catalog.license`, except its personal-data statement; `dct:spatial`, `dct:accrualPeriodicity`, `dcat:keyword`, `r5r:applicableLegislation` come from the Endpoint's `spec.catalog`, the publisher and contact point from it or else from its Project's `spec.catalog` (section 6). A property a profile requires and the Endpoint does not declare is left out, never invented, and the conformance suite names it.

- **The personal-data statement.** No licence says whether a dataset holds personal data, so the Endpoint states it in `spec.catalog.personalData`: `contains` or `does-not-contain`. Unset, the property is left out and the conformance suite names it. The code lists, read on 2026-10-18:

  | `personalData` | DCAT-AP-SK `leg:personalDataContainmentType` | DCAT-AP-CZ `pu:osobní-údaje` |
  |---|---|---|
  | `contains` | `https://data.gov.sk/def/personal-data-occurence-type/1`, "Obsahuje osobné údaje" | `https://data.gov.cz/podmínky-užití/obsahuje-osobní-údaje/` |
  | `does-not-contain` | `https://data.gov.sk/def/personal-data-occurence-type/2`, "Neobsahuje osobné údaje" | `https://data.gov.cz/podmínky-užití/neobsahuje-osobní-údaje/` |

  The Slovak labels are the code list `https://data.gov.sk/set/codelist/personal-data-occurence-type` as `https://data.slovensko.sk/codelists?keys[]=https://data.gov.sk/set/codelist/personal-data-occurence-type` serves it. The Czech IRIs are the individuals of `dcat-ap-cz-podmínky-užití/2025-08-12/model.owl.ttl` in `datagov-cz/otevrene-formalni-normy` (commit `fa3cb6a`). The Slovak list has a third code, `/3` "Môže obsahovať osobné údaje (nie je špecifikované)", which the national catalogue refuses on input (`slovak-egov/nkod-portal`, `DistributionLicenceInput.cs` at `71b5ea98`); the Czech list has no such value, so neither profile offers it.

Decided (worker-2, T-4499, owner may override): a profile is a query parameter, not content negotiation by `Accept-Profile`, because a harvester is configured with one URL. The other options were one URL per profile, or the profile as an `Accept-Profile` header.

## 6. One catalogue per publisher

A national catalogue registers one local catalogue per publisher: data.slovensko.sk a body by its IČO, data.gov.cz a body of the RPP register. One installation hosts the data of several publishers (a city, its region, a state network), each a Project naming its body in `spec.domain` ([Architecture/03](../Architecture/03-domain-model.md)), so the gateway serves a catalogue per Project that names its publisher (EP-127):

- **Address.** `https://api.{domain}/catalog/{project}.jsonld` and `https://api.{domain}/catalog/{project}.ttl`, anonymous, with `?profile=` as in section 5. The catalogue's IRI is `https://api.{domain}/catalog/{project}`. A project that names no publisher, a project without a public Endpoint, and a name that is no project all answer the same 404.
- **Content.** One `dcat:Catalog` holding the record of every Endpoint of the project whose audience is `public`, as the anonymous caller reads it, and nothing of any other project; an App's own Endpoint (`app-{name}`, which the Portal writes for the App) re-serves a space for that App and is in no catalogue, this one or the installation's, though its record stays at its address (T-4612); each dataset is an IRI that dereferences to its complete record, which is the "DCAT-AP Dokumenty" interface both national catalogues harvest.
- **The publisher.** `Project.spec.catalog.publisher`: `name` per language and `uri`, the body's registry IRI: `https://data.gov.sk/id/legal-subject/{IČO}` for a Slovak body, `https://rpp-opendata.egon.gov.cz/odrpp/zdroj/orgán-veřejné-moci/{IČO}` for a Czech one. `uri` is required here, because a catalogue's publisher is an IRI in both profiles. The catalogue names it as `dct:publisher`, and every record of the project whose Endpoint names no publisher of its own carries it too.
- **The contact point.** `Project.spec.catalog.contactPoint`, a desk's `name` and role `email` (EP-80), on the catalogue and on every record whose Endpoint names none.
- **Title, description, homepage.** The Project's `metadata.title` and `metadata.description` per language, and `https://{spec.domain}/` as `foaf:homepage` when the project names a domain.

```yaml excerpt
kind: Project
metadata:
  name: banskabystrica
  title: { sk: "Mesto Banská Bystrica", en: "City of Banská Bystrica" }
spec:
  domain: banskabystrica.sk
  catalog:
    publisher:
      name: { sk: "Mesto Banská Bystrica", en: "City of Banská Bystrica" }
      uri: https://data.gov.sk/id/legal-subject/00313271
    contactPoint: { name: "Oddelenie informatiky", email: otvorenedata@example.org }
```

The installation's catalogue `/catalog.jsonld` (EP-84) stays: it lists every public Endpoint under the Organization and is what data.europa.eu or another CKAN harvests. Registering a publisher's URL with its national catalogue is the publisher's step ([User-Guide/13](../User-Guide/13-national-open-data-portals.md)).

## 7. Safe and reusable records

These rules hold for every profile, so a record meeting a national catalogue never says more than the Endpoint's public description:

- **Only public metadata.** A record carries no internal id, no person's e-mail and no name of a private space or project; a restricted Endpoint's record names what the anonymous caller may know and no more (EP-27).
- **Publisher and contact point are the publisher's.** The Endpoint's or its Project's `spec.catalog` enters them; the contact is a desk, never a person read from Keycloak.
- **Licences only from the EU licence table** (`http://publications.europa.eu/resource/authority/licence`), and the national terms-of-use node (`leg:TermsOfUse`, `pu:Specifikace`) is derived from that licence, never typed separately (EP-78).
- **Profiles are data.** The shapes and the mapping table are the profile; the code that writes a record reads them and holds no profile-specific branch.

## 8. Requirements

- **EP-27** [H][P][A] — the Endpoint's DCAT-AP 3.0 record ([Requirements/endpoints](../Requirements/endpoints.md)).
- **EP-46** [P][A] — the schema artifacts the record lists.
- **EP-68** [P][A] — every artifact a `dcat:distribution` with checksum and `dcterms:conformsTo`.
- **EP-78** [H][P][A] — the licence the record and the CKAN dataset carry.
- **EP-84** [H][P][A] — the installation's catalogue for harvesters.
- **EP-127** [H][P][A] — one catalogue per publisher, in the national profiles.

## Related

- [API/02-endpoint-representations](02-endpoint-representations.md) — the record itself and every representation an Endpoint serves.
- [Architecture/21-open-data-catalogue](../Architecture/21-open-data-catalogue.md) — the catalogue the record is published to.
- [Requirements/endpoints](../Requirements/endpoints.md) — EP-27, EP-46, EP-68, EP-78, EP-84, EP-127.
- [User-Guide/13-national-open-data-portals](../User-Guide/13-national-open-data-portals.md) — registering a publisher's catalogue with data.slovensko.sk and data.gov.cz.
