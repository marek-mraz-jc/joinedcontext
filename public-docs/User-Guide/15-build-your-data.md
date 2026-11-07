---
sidebar_position: 17
title: Build Your Data, End to End
description: One feed from nothing to entities you can read, in the order the Portal needs it (context space, data model, endpoint, data source, pipeline), with the defaults each step fills in and the mistake people make at each step.
routes:
  - /projects/$project/spaces/upload
---

# Build Your Data, End to End

This page builds one feed from nothing: Helsinki's city bike stations, read from the operator's open GBFS feed and published through an endpoint of your organization. It takes the five kinds in the order the Portal needs them, says for each one what it is for, which page writes it, the YAML that page proposes, the defaults the platform fills in, and the mistake people make there. A second, shorter example loads a CSV file. It is written for data stewards; the chapters it links to describe each kind in full.

Every step is a change: you propose it, an approver approves it, and the next step can name it once it is live. On dev a change is live about a minute after its approval. The live journey `build-your-data.spec.ts` replays this page on dev as `demo.steward@hel.fi`, reads the stations back through the endpoint, removes everything again, and takes the screenshots below.

| Step | Kind | What it is for |
|---|---|---|
| 1 | Context Space | the place the entities live and the rules for keeping them |
| 2 | Data Model | the types and attributes the space accepts |
| 3 | Endpoint and its Policies | who reads the space, in which formats, and who writes it |
| 4 | Data Source | where the platform fetches from |
| 5 | Pipeline | how often it fetches, and what one record becomes |

The order matters. A space comes first, because the Portal creates a data model inside its space. The endpoint comes before the pipeline, because a pipeline writes through an endpoint.

## 1. Context Space

A context space holds the entities of one subject and says how long they are kept.

Open **Context Spaces**, choose **New Context Space**, enter `guide-bikes` in **Name** and `City bike stations` in **Title**, leave **Data model** empty, choose **Check**, then **Propose change**. The page proposes:

```yaml title="projects/helsinki/spaces/guide-bikes/space.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: guide-bikes
  namespace: helsinki
  title: { en: "City bike stations" }
spec:
  defaultLocale: en
  isSandbox: false
```

What the platform fills in when you set nothing:

| Setting | Default | What it means |
|---|---|---|
| Data model | an empty model named after the space | created with the space; step 2 gives it its types |
| Current values | kept until something removes them | a station the feed stops listing stays in the space |
| History | none until a rule names the type | the space keeps current values only; set **History** on the space's **Retention** tab to keep a type's past |
| Data quality | no rules | only the model's own constraints are checked |
| Entity ids | `urn:ngsi-ld:{Type}:{id}` | the id the pipeline writes is the entity's identity in this space |

The space's page shows each of these: **Data model**, **Data quality** and **Retention**, where the **History** column reads **Platform default** until you set a rule.

![The new space guide-bikes: its data model, no entities yet, no quality rules, and the retention section](img/en/build-1-context-space.png)

**The mistake:** expecting the space to forget old data on its own. It keeps every current value until an end time, a space rule or pipeline expiry removes it. Decide that now, with [Retention & Expiry](./14-retention-and-expiry.md); for this feed the defaults are right, because stations rarely close.

## 2. Data Model

The data model is the list of types the space accepts and the attributes each type has. A write of a type the model does not declare is refused, and so is a value outside its constraints.

Open the space's model with **Edit model**, add the class `BikeHireDockingStation` with two slots, and propose. The editor writes the model's LinkML source:

```yaml title="projects/helsinki/spaces/guide-bikes/datamodels/guide-bikes.linkml.yaml"
id: https://hel.fi/models/helsinki/guide-bikes
name: guide-bikes
description: The types of the guide-bikes space, one class per entity type (DM-61).
prefixes:
  linkml: https://w3id.org/linkml/
  ngsi-ld: https://uri.etsi.org/ngsi-ld/
  helsinki: https://hel.fi/models/helsinki/guide-bikes/
default_prefix: helsinki
default_range: string
imports:
  - linkml:types
  - ngsi-ld-core
slots:
  name:
    description: The station's name as the operator writes it.
    required: true
  totalSlotNumber:
    description: How many bikes the station docks when full.
    range: integer
    minimum_value: 0
classes:
  BikeHireDockingStation:
    description: A city bike station.
    is_a: Entity
    slots:
      - name
      - totalSlotNumber
```

The fields that matter: the class name, which every entity of this type carries as its `type`; `required`, which refuses an entity without the attribute; `range` and `minimum_value`, which refuse a wrong value. `is_a: Entity` brings what every NGSI-LD entity has, `id`, `type` and `location`, so you do not declare them.

Defaults: a slot without `range` is text (`default_range: string`), a slot without `required` is optional, and the model starts as version `0.1.0` in the **Draft** state. A draft model is enough for your own space; publish a version when other projects should build on it ([Data Models](./03-data-models.md#6-deciding-a-model-versions-lifecycle)).

![The data model guide-bikes in its space, in the Draft state](img/en/build-2-data-model.png)

**The mistake:** naming the type differently in the model and in the pipeline (`BikeStation` here, `BikeHireDockingStation` there). Every write is then refused as an unknown type. Copy the class name from the model into the pipeline.

## 3. Endpoint and Its Policies

An endpoint is the address the space is read through. Policies say who may do what with the space: one lets the members of the project read the stations, one lets the pipelines write them.

Open **Policies** and propose the two policies; the form is described in [Endpoints & Sharing](./05-endpoints-and-sharing.md#6-deciding-who-may-read-and-write-policies). Then open **Endpoints**, choose **New endpoint**, enter `guide-bikes` in **Name**, pick `guide-bikes` in **Context Space**, choose **Generate slug**, set **Audience** to *Organization*, tick *NGSI-LD API* and *CSV* under **Representations**, and propose. Together they write:

```yaml title="projects/helsinki/policies/guide-bikes-pipelines-write.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Policy
metadata:
  name: guide-bikes-pipelines-write
  namespace: helsinki
  title: { en: "The pipeline writes the bike stations" }
spec:
  contextSpaceRef: { kind: ContextSpace, name: guide-bikes }
  assigner: did:web:{orgDomain}
  assignee: { kind: serviceAccount, id: pipelines }
  operations: [upsertBatch, createBatch, queryBatch]
```

```yaml title="projects/helsinki/policies/guide-bikes-read.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Policy
metadata:
  name: guide-bikes-read
  namespace: helsinki
  title: { en: "The project's members read the bike stations" }
spec:
  contextSpaceRef: { kind: ContextSpace, name: guide-bikes }
  assigner: did:web:{orgDomain}
  assignee: { kind: group, id: helsinki }
  operations: [retrieveOps]
  information:
    - entities:
        - type: BikeHireDockingStation
```

```yaml title="projects/helsinki/endpoints/guide-bikes.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: guide-bikes
  namespace: helsinki
  title: { en: "City bike stations" }
spec:
  contextSpaceRef: guide-bikes
  slug: guidebikesstationsdemoabcd
  audience: organization
  enabledRepresentations: [ngsi-ld, csv]
```

The fields that matter:

- **Audience**: *Organization* lets every signed-in member and service account of the organization call it; *Selected projects* narrows that to the projects you tick; *Public* opens it to anyone and needs the publisher role to approve.
- **Representations**: without a list the endpoint serves every representation except SensorThings; a list serves only what it names. The MCP surface for AI assistants is on either way, behind the same policies, until the manifest says `mcp: false`.
- **Slug**: the address. It never changes, so a link you hand out keeps working; generate it rather than typing a word, because a guessable address is a weaker one.
- **Policy**: the read policy decides who reads (`assignee`: the group `helsinki` is the project's members) and which types and attributes they get (`information`), and the endpoint's **Hidden attributes** can only take more away. The write policy names the `pipelines` service account, the identity every pipeline of the project writes as. A policy grants a right, so its change is in the red lane: the approver types the policy's name back before **Approve** works.

Defaults: no rate limit, no caching, nothing hidden, no catalogue entry. Add those on the endpoint's page when the endpoint is public ([Endpoints & Sharing](./05-endpoints-and-sharing.md)).

![The endpoint guide-bikes with its audience, address and representations](img/en/build-3-endpoint.png)

**The mistake:** an endpoint without a read policy. The endpoint exists, its links answer, and every read is empty or refused. The space's page says why: its policies grant no operation a view serves. Propose the read policy with the endpoint, never after. The same refusal comes from a read policy granted to a group the readers are not in: a grant to the organization's domain (`hel.fi`) instead of the project's group (`helsinki`) answers every read with 403 `AccessDenied`.

## 4. Data Source

A data source says where the platform fetches from and how; it holds no schedule and no mapping, so several pipelines can read one source.

Open **Data sources**, choose **New data source**, pick *HTTP endpoint* as **Type**, enter `guide-bikes-stations` in **Name**, the feed's address in **URL**, `GET` as **Method** and `15s` as **Timeout**, choose **Check**, then **Propose change**:

```yaml title="projects/helsinki/datasources/guide-bikes-stations.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: DataSource
metadata:
  name: guide-bikes-stations
  namespace: helsinki
  title: { en: "City bike stations (GBFS)" }
spec:
  type: http
  http:
    url: https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/station_information.json
    verb: GET
    headers: { Accept: application/json }
    timeout: 15s
```

**Check** fetches the address once and shows what came back: here one JSON document with a `data.stations` list, one item per station with `station_id`, `name`, `lat`, `lon` and `capacity`. Read it before you write the pipeline; the mapping in step 5 is written against exactly this shape.

![The data source guide-bikes-stations with its address and method](img/en/build-4-data-source.png)

**The mistake:** pasting a key or a token into the **URL** or a header. This feed is open and needs none. A feed that needs one gets a **Secret name** and a **Key in the secret** under **Credentials**: the source holds the reference, the secret store holds the value, and nothing secret ends up in the repository or in a log.

## 5. Pipeline

A pipeline reads a data source on a schedule, turns each record into NGSI-LD entities and writes them through an endpoint.

Open **Pipelines** and start a new one: pick `guide-bikes-stations` as **Data source**, set **Execution** to *Automatic, from the period* with `15m` as **Period**, paste the mapping into **Bloblang mapping**, pick `guide-bikes` as **Target endpoint**, set **Write mode** to *Create or merge*, run the test on the sample, and propose:

```yaml title="projects/helsinki/pipelines/guide-bikes-stations.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: guide-bikes-stations
  namespace: helsinki
  title: { en: "City bike stations from GBFS" }
spec:
  class: auto
  period: 15m
  source:
    dataSourceRef: { kind: DataSource, name: guide-bikes-stations }
  compute:
    kind: bloblang
    bloblang: |
      root = this.data.stations.map_each(s -> {
        "id": "urn:ngsi-ld:BikeHireDockingStation:station-" + s.station_id.string(),
        "type": "BikeHireDockingStation",
        "name": { "type": "Property", "value": s.name },
        "totalSlotNumber": { "type": "Property", "value": s.capacity },
        "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [ s.lon, s.lat ] } }
      })
  output: { mode: upsert, type: BikeHireDockingStation }
  targetEndpoint: urn:ngsi-ld:Endpoint:hel.fi:helsinki-guide-bikes:guide-bikes
  quotas:
    maxMemoryMb: 64
    cpuMillicores: 100
```

The fields that matter:

- `id`: built from the feed's own `station_id`, so each run updates the same 455 entities. An id that changes per run (a counter, a timestamp) adds a new entity every fifteen minutes.
- `coordinates`: longitude first, then latitude, as GeoJSON wants; the feed gives `lat` and `lon` separately.
- **Period**: `15m` here. *Automatic, from the period* runs a period of 30 seconds or more as a scheduled job and a shorter one as an always-running stream; pick *On a schedule* only when you need a cron time such as `10 0 * * *`.
- **Write mode**: *Create or merge* keeps attributes other writers set; *Replace the whole entity* drops them.

Defaults: no expiry (an entity the feed stops sending stays), no end time on the entities, history kept by the space's rule (28 days here), and the quotas above when you set none. When a station leaves the feed and should leave the space, switch on **Remove entities the source no longer sends**; when each record carries its own end, write `expiresAt` in the mapping. [Retention & Expiry](./14-retention-and-expiry.md#3-which-one-do-i-pick) says which to pick.

![The pipeline guide-bikes-stations with its period and target endpoint](img/en/build-5-pipeline.png)

**The mistake:** writing coordinates as `[lat, lon]`. Every station then lands in the sea off Iran, the write still succeeds, and the map is wrong without an error. Check one entity on the map after the first run.

## 6. Read It Back

The pipeline runs once as soon as it is live, then every 15 minutes. The space's page counts the stations it now holds (455 on 2026-10-20). Your Portal session is a member of the organization, so the endpoint's links on its page open the data directly, and **Explore** shows the stations on a map. A program reads the same data with a token of a service account of the organization:

```bash
curl -s -H "Accept: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  "https://portal.dev.joinedcontext.com/api/endpoint/guidebikesstationsdemoabcd/ngsi-ld/v1/entities?type=BikeHireDockingStation&limit=2"
```

![The space guide-bikes after the pipeline's first run, its model declaring BikeHireDockingStation with name and totalSlotNumber](img/en/build-6-read-back.png)

## 7. A Second Example: Loading a CSV File

A file needs no data source and no pipeline: the Portal reads it in your browser and writes the rows as you.

1. On the space's page, choose **Upload CSV**, or open it from **Context Spaces**.
2. **Choose a CSV file** of up to 50 MB. The Portal ranks the columns against the Smart Data Models types.
3. Under **What the rows are**, take the type it offers, or **My own model** with a **Type name** such as `Garage`.
4. Pick the **Context Space**, then **Propose the change**. One change proposes what the rows need: the type in the space's model, the endpoint and your right to write.
5. Once an approver has approved it and it is live, the page offers to write the rows. Writing the same file again updates the same entities, because each row's id comes from the file.

Defaults: uploaded rows never expire and their history follows the space's rule, 28 days unless the space says otherwise. A file whose rows have an end (a permit valid until a date) needs that end in each row; the recipe is in [Retention & Expiry](./14-retention-and-expiry.md#11-recipe-data-imported-from-a-file).

**The mistake:** uploading a new version of a file whose rows lost an id column. Every row then gets a new id, and the space holds both versions. Keep a stable id column, or remove the old rows first.

## Related

- [Getting Started](./01-getting-started.md): the same steps as a first tour of the Portal.
- [Organizations, Projects & Context Spaces](./02-organizations-projects-spaces.md): spaces, copies and completing a space from a sample.
- [Data Modeling & The LinkML Editor](./03-data-models.md): the model editor, imports and versions.
- [Pipelines & Ingestion](./04-pipelines.md): execution classes, the studio and blueprints.
- [Endpoints, Data Sharing & Integration](./05-endpoints-and-sharing.md): audiences, projections, policies and subscriptions.
- [Retention & Expiry](./14-retention-and-expiry.md): every way data is removed, and which one wins.
