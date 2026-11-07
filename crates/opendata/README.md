# `opendata` (`jc-opendata` / `catalogue`) — DCAT-AP Open Data Portal & Harvester API

## Scope
`opendata` (`catalogue`) serves the open data catalogue per organization and across the deployment:
- Renders dataset, distribution, data service, and vocabulary pages on demand.
- Executes full-text and faceted search directly in PostgreSQL (`tsvector` GIN index on schema `catalogue`) with zero Solr or Redis processes.
- Computes European Data Portal Metadata Quality Assessment (MQA) scores.
- Exposes RDF/Turtle, JSON-LD, RDF/XML, Hydra-paged DCAT-AP feeds, OAI-PMH 2.0 `ListRecords`, OGC API Records, and the CKAN Action API v3 read subset (`package_search`, `package_show`, `organization_list`, `group_list`, `tag_list`) for national open-data harvesters (e.g. NKOD / data.gov.cz, data.gov.sk, data.europa.eu).

## Public API
- HTTP: `/catalog.{ttl,jsonld,rdf}`, `/datasets/{slug}`, `/oai`, `/api/3/action/{package_search,package_show,organization_list,group_list,tag_list}`, `/ogc/records/*`
- gRPC / In-Process: `jc_opendata_api::CatalogueService`

## Standalone Binary
`jc-opendata`

## Configuration
- `DATABASE_URL` (role `jc_catalogue`, schema `catalogue`)
- `OPENDATA_LISTEN_ADDR` (default `127.0.0.1:8085`)

## Standards Implemented
- DCAT-AP 3.0.1, DCAT-AP HVD (High-Value Datasets), GeoDCAT-AP 3.0.0
- OAI-PMH 2.0
- OGC API Records Part 1
- CKAN Action API v3 (harvester read subset)

## Origin Today
Replaces **CKAN (Python), Apache Solr (Java), Redis, and CKAN DataStore**.
