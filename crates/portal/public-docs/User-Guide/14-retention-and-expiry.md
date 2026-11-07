---
sidebar_position: 16
title: Retention & Expiry
description: How long a context space keeps current values and history, every way data is removed, which one wins, and recipes for buses, alerts, events, single fields, sensors and imported files.
---

# Retention & Expiry

Data in a context space has two parts that age differently: the *current value* of each entity (where bus 1234 is now) and its *history* (every position it reported). This page lists every way the platform removes either part, which one wins when several apply, what a new space does when you set nothing, and gives a recipe to copy for live vehicles, alerts, air-quality sensors and data imported from a file. It is written for data stewards and integration engineers who build pipelines and spaces.

## 1. What a New Space Does

A space you create keeps:

| Part | Kept | Until you change it with |
|---|---|---|
| Current values | for ever: nothing removes an entity on its own | a pipeline's **Lifetime**, an end time on the entity (`expiresAt`), a space rule **Current values**, or pipeline expiry |
| History | none: a type no rule names keeps its current values only | a space rule **History**, the **All other types** row, or an entity's own `historyRetention` |
| Pipeline expiry | off | **Remove entities the source no longer sends** on the pipeline |

The Helsinki sample project also carries an older settings file that says history is kept 90 days. Nothing reads that value: a type no rule names keeps no history, as the table says, and the stale value is being removed.

## 2. Every Way Data Is Removed

| Mechanism | Removes | Where you set it |
|---|---|---|
| Pipeline **Lifetime** | the whole entity, or only the fields you pick, when the end time it writes passes | the pipeline editor, **Lifetime** step; saved as `spec.lifetime` |
| Entity `expiresAt` | the whole entity at that moment; reads stop showing it at once | written by the pipeline's **Lifetime**, or by a mapping line |
| Attribute `expiresAt` | one attribute of an entity | written by the pipeline's **Lifetime** (**Only some fields expire**), or by a mapping line |
| Space rule **Current values** | an entity of that type not updated for the duration, unless it has its own `expiresAt` | space, **Retention**, per type |
| Pipeline expiry | entities of the named types not updated for the window, checked once an hour | the pipeline, **Remove entities the source no longer sends** |
| Space rule **History** | history rows of that type older than the duration; `off` records none, **Keep forever** keeps all and asks why | space, **Retention**, per type |
| **All other types** row | history of every type without its own rule | space, **Retention** |
| Platform default | history of everything nothing else rules, after 28 days | the platform, not per space |
| Entity `historyRetention` | history of that one entity, a duration or `off` | in the data, on the entity |
| Recording on change | nothing: it writes fewer history rows, only when a value moves | the space manifest, per type or attribute |
| Delete and erasure | what a person deletes; an erasure removes a data subject's values everywhere | a person's action, approved |

### Which one wins

For current values, an `expiresAt` on the entity wins over the space rule, whether a **Lifetime** or a mapping line wrote it: the space rule only touches entities without one. Pipeline expiry is a separate sweep and removes what it names regardless of the space rule.

For history, the most specific setting wins, in this order:

1. an erasure;
2. the entity's own `historyRetention`;
3. the space rule of the entity's type (**Keep forever** there holds back every level below);
4. the space's **All other types** row;
5. without any of these the type keeps no history: history is opt-in.

Personal data adds a limit of its own. A data model that marks an attribute as personal must state its maximum retention, and the Portal refuses the model without it. The platform does not yet delete personal values when that maximum passes: until it does, give a type that holds personal data a space rule for **History** no longer than its stated maximum, and use erasure for a person's request.

## 3. Which One Do I Pick

Start from what the source tells you:

- *The source says when an item ends* (a vehicle position is stale after minutes, an alert has a valid-to time, a permit has an end date): give the pipeline a **Lifetime**. Every write renews it, and the entity goes the moment it passes.
- *Only one field goes stale, the item stays* (a vehicle's position, a parking space's occupancy): a **Lifetime** with **Only some fields expire**.
- *The source sends the whole list every run and stops listing an item* (stations, a register, a catalogue export): use `sourceDeletes` with `snapshot` ([section 13](#13-when-the-source-deletes-a-record)), or pipeline expiry with a window a few runs long.
- *Every entity of a type goes stale after a fixed time without an update, whoever wrote it*: a space rule **Current values** for that type.
- *History grows too large*: shorten the type's **History**, set it to **Off** for types where only now matters, or record on change so a sensor that reads the same value all night writes one row an hour.

Two settings that look alike behave differently. A space rule **Current values** removes an entity because nothing updated it. Pipeline expiry removes it because the pipeline stopped sending it. For a feed that only sends what changed, a sensor reading or a change stream, use neither: a quiet entity there is not gone, it has nothing new to say.

### Set a space's rules

#### By hand

1. Open the space at `/projects/helsinki/spaces/air-quality`.
2. In the **Retention** section, click **Change retention**.
3. For each type, set **Current values** and **History** as a number and a unit, or pick **Off: no history** or **Keep forever**. The **All other types** row sets history for every type without a rule of its own. For a type that keeps history, **Rows of history** says which rows the broker writes: **Newer observations** (the default) writes a row only when `observedAt` is newer than the last row, **Every observation** writes one for each new `observedAt`, as a count per 15 minutes, and **Only changes** writes one when a value changes and at least once per chosen interval, as the free bikes of a station.
4. Click **Propose**. When the new rules would delete data at once, the dialog **Delete data of air-quality for good?** lists how many current values and history records go; confirm with **Propose and delete**.
5. An approver merges the proposal in Approvals; the rules apply from then on.

Recording on change has no form field: set it in the space manifest as the recipes below show, and propose the file as a change.

## 4. The Lifetime Step of the Pipeline Editor

The **Lifetime** step of the pipeline editor offers five choices. The editor saves the choice as `spec.lifetime` in the pipeline manifest, beside the mapping, so it reads it back after you edit the mapping by hand.

| Choice | What it writes | Use it for |
|---|---|---|
| **Keep entities** | no end time | sensors, registers, anything that stays until the source or a person removes it |
| **From a field** | the field's time, plus a margin, at most a cap after the write | vehicles (observation time), alerts (valid-to time) |
| **After each write** | the write time plus a duration, renewed at every write | anything that is gone when the feed stops mentioning it |
| **On a fixed date** | one date and time for every entity the pipeline writes | an event, a season, a campaign |
| **Only some fields expire** | an end time on each field you pick; the entity stays | a position or an occupancy that is stale after minutes |

Before **Propose change** is enabled, click **Check**. The Portal runs the sample records through the same rules the write path uses: the space's data model, what the target endpoint lets the pipeline write, and the lifetime each record gets. Each problem names the record, the field and the rule. **Propose change** stays disabled until the check of this draft is green, and the Portal refuses a proposal without one, even one sent by `jcctl` or the API.

The editor refuses a field that is not in the written type, a fixed date that has already passed, and a duration under one minute or over ten years. A record whose field is empty or not a time is written without an end time and counted in the pipeline's problems, so a feed that stops sending the field shows up instead of filling the space.

A lifetime needs no extra permission. The pipeline never deletes anything for it: the broker removes the entity or field when its end time passes, which the endpoint's write permission already allows the pipeline to set.

Until your Portal shows the **Lifetime** step, write the end time in the mapping with the line each recipe gives. Do not use both for the same type: the Portal refuses a pipeline whose mapping sets `expiresAt` and whose **Lifetime** covers the same entity or field.

## 5. Recipe: Buses and Live Vehicles

A vehicle feed sends a position every second while the vehicle runs and nothing when it parks. Give each vehicle an end time five minutes after its last observation, so a bus that stopped reporting is gone five minutes later, and keep a light history.

In the editor's **Lifetime** step, choose **Whole entity expires** and **From a field**, pick `observedAt` as **Field** and enter `5` `minutes` as **Plus**. The manifest gets:

```yaml title="projects/helsinki/pipelines/vehicle-positions/pipeline.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: vehicle-positions
  namespace: helsinki
  title: { en: "Live vehicle positions" }
spec:
  class: auto
  source:
    dataSourceRef: { kind: DataSource, name: vehicle-positions }
  targetEndpoint: urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all
  lifetime:
    - type: Vehicle
      entity: { from: observedAt, plus: PT5M }
```

`observedAt` is the newest observation time among the vehicle's fields. A position observed at `2026-10-20T15:00:00.123Z` gets `expiresAt` `2026-10-20T15:05:00Z`. A separate cleanup pipeline that reads the vehicles back and deletes the old ones is no longer needed.

Without the **Lifetime** step, add one line to the mapping after it builds the entity. `tst` is the observation time the high-frequency positioning feed sends:

```coffee title="bento.yaml, the mapping of the vehicle pipeline"
root.expiresAt = (this.tst.ts_unix() + 300).ts_format("2006-01-02T15:04:05Z", "UTC")
```

For the history, record a position only when the vehicle moved 50 metres, and at least every five minutes while it stands, and keep a week:

```yaml title="projects/helsinki/spaces/transit-live/space.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: transit-live
  namespace: helsinki
  title: { en: "Live vehicles" }
spec:
  isSandbox: false
  dataModelRef: { kind: DataModel, name: transit }
  defaultLocale: en
  retention:
    - entityType: Vehicle
      history: P7D
      record: onChange
      heartbeat: PT5M
      minDistance: 50
```

When only the live map matters, write `history: "off"` instead and leave out the three recording lines: the space then keeps no history of vehicles at all.

## 6. Recipe: Alerts and Roadworks

An alert or a roadwork carries the time it stops applying. Use it as the entity's end time, and cap it, so an alert with a mistyped year does not stay for decades.

In the **Lifetime** step, choose **Whole entity expires** and **From a field**, pick `validTo` as **Field**, leave **Plus** at `0` and set **At most after the write** to `30 days`:

```yaml title="projects/helsinki/pipelines/service-alerts/pipeline.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: service-alerts
  namespace: helsinki
  title: { en: "Service alerts" }
spec:
  class: auto
  period: 5m
  source:
    dataSourceRef: { kind: DataSource, name: service-alerts }
  targetEndpoint: urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all
  lifetime:
    - type: Alert
      entity: { from: validTo, notAfter: P30D }
```

`2026-10-21T06:00:00+03:00` becomes `2026-10-21T03:00:00Z`. A `validTo` of `2062-10-21` becomes 30 days after the write, and every write of the alert moves that cap on. An alert without `validTo`, or with a value that is not a time, ends 30 days after the write, the **At most after the write** value, and is counted in the pipeline's problems. Without that value such an alert gets no end time and stays until the source withdraws it. An alert whose `validTo` already passed is written and disappears at once. This replaces a cleanup pipeline that deleted ended alerts.

Without the **Lifetime** step, the mapping line has no cap:

```coffee title="bento.yaml, the mapping of the alerts pipeline"
root.expiresAt = this.validTo.ts_format("2006-01-02T15:04:05Z", "UTC").catch(deleted())
```

When the source withdraws alerts by leaving them out of its list instead, use pipeline expiry as in section 9.

## 7. Recipe: An Event That Ends on a Date

The city publishes road closures for a Christmas market. Every closure ends when the market closes, whatever the feed says, so give every entity the pipeline writes the same end time.

In the **Lifetime** step, choose **Whole entity expires** and **On a fixed date**, and enter the date and time in the project's time zone. The editor shows it in UTC and refuses a date that has already passed:

```yaml title="projects/helsinki/pipelines/market-closures/pipeline.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: market-closures
  namespace: helsinki
  title: { en: "Christmas market road closures" }
spec:
  class: auto
  period: 1h
  source:
    dataSourceRef: { kind: DataSource, name: market-closures }
  targetEndpoint: urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all
  lifetime:
    - type: RoadClosure
      entity: { at: "2026-12-31T22:00:00Z" }
```

At midnight in Helsinki on New Year's Eve every closure of this pipeline is gone. To run the market again next year, change the date and propose the pipeline again.

## 8. Recipe: Only the Position Expires

A parked bus keeps its line, its operator and its last delay, but its position is only true while it reports. Let the position go five minutes after the last write and keep the vehicle.

In the **Lifetime** step, choose **Only some fields expire**, pick `location` as **Field**, choose **After each write** and enter `5` `minutes`:

```yaml title="projects/helsinki/pipelines/vehicle-status/pipeline.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: vehicle-status
  namespace: helsinki
  title: { en: "Vehicle status, position while reporting" }
spec:
  class: auto
  source:
    dataSourceRef: { kind: DataSource, name: vehicle-positions }
  targetEndpoint: urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all
  lifetime:
    - type: Vehicle
      attributes:
        - { names: [location], rolling: PT5M }
```

Each write renews the position's end time. Five minutes after the last one the vehicle has no position, and the map stops drawing it, while a query by line still finds the vehicle. Pick several fields in one row when they go stale together, and add a row for fields with another duration. A field lifetime combines with a whole-entity lifetime and with pipeline expiry.

## 9. Recipe: A Full List Such as Stations

A station list arrives whole every five minutes. A station missing from it for two days has closed:

```yaml title="projects/helsinki/pipelines/city-bike-stations/pipeline.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: city-bike-stations
  namespace: helsinki
  title: { en: "City-bike stations, every five minutes" }
spec:
  class: auto
  period: 5m
  source:
    dataSourceRef: { kind: DataSource, name: city-bike-stations }
  targetEndpoint: urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all
  expiry:
    after: 2d
    types: [BikeHireDockingStation]
```

The window is whole hours or days, from `1h` to `365d`, for one to twenty types. Make it several runs long, so one failed run does not empty the space. The Portal refuses expiry while a second pipeline writes into the same space, and while no Policy lets the pipeline delete those types. The steps in the Portal are in [Pipelines & Ingestion](./04-pipelines.md#7-removing-entities-the-source-dropped).

## 10. Recipe: Air-Quality Sensors

A sensor's current value stays until the next reading, so set no current-value rule. Keep 30 days of history, write a row only when a value moves, and still write one row an hour so a flat line is visible:

```yaml title="projects/helsinki/spaces/air-quality/space.yaml"
apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: air-quality
  namespace: helsinki
  title: { en: "Air quality" }
spec:
  isSandbox: false
  dataModelRef: { kind: DataModel, name: air-quality }
  defaultLocale: en
  retention:
    - entityType: AirQualityObserved
      history: P30D
      record: onChange
      heartbeat: PT1H
      attributes:
        pm10: { deadband: 1 }
        no2: { deadband: 2 }
    - entityType: "*"
      history: P90D
```

`deadband` is in the attribute's own unit: here a PM10 reading is recorded when it moves by more than 1 µg/m³. An attribute the rule does not name records on any change. The last rule is the **All other types** row: every other type in this space keeps 90 days of history instead of the platform's 28.

## 11. Recipe: Data Imported From a File

To load a CSV or JSON file into a space, complete the space from the file (see [Organizations, Projects & Context Spaces](./02-organizations-projects-spaces.md#4-completing-a-space-from-a-sample)): the Portal drafts a data source and a pipeline that read it. The **Import** page under Export does something else: it imports configuration bundles, never entity data.

Imported entities never expire on their own, and the space's history setting applies to them like to any other write. When rows of the file have an end, add a column, for example `validUntil` with dates such as `2027-12-31`, and pick it in the **Lifetime** step as **From a field**, or set it as the end time in the pipeline's mapping:

```coffee title="bento.yaml, the mapping of the import pipeline"
root.expiresAt = this.validUntil.ts_parse("2006-01-02").ts_format("2006-01-02T15:04:05Z", "UTC").catch(deleted())
```

A row with an empty `validUntil` stays. Pipeline expiry does not suit a one-off file: the file is read once, so after the window every entity would look abandoned and be deleted.

## 12. Recipe: Personal Data

Mark the attribute as personal in the data model, with its legal basis, purpose and maximum retention; the Portal refuses the model without them. Until the platform deletes personal values by that maximum on its own, set the type's **History** in the space no longer than the maximum, and send a person's erasure request through erasure, which removes their values from the current state, the history and every copy.

## 13. When the Source Deletes a Record

A record that an open-data portal withdraws must leave your space too, and only when the source really removed it. Tell the pipeline how its source shows a removal, in the pipeline manifest under `sourceDeletes`:

| Choice | What happens to a record the source removed | Pick it when |
|---|---|---|
| `never` | it stays in the space with its last value | you keep an archive, or the source never removes records |
| `snapshot` | it goes after two complete runs that no longer list it | the source returns its whole list every run: a catalogue, a register, a CSV export |
| `events` | it goes when the mapped record matches `deletedWhen` | the source marks removals, for example `status` set to `withdrawn` |
| `lease` | it goes when the source stops sending it, by a rolling **Lifetime** | the source only sends what is current, such as live buses |

```yaml excerpt title="projects/bbsk/pipelines/datasets/pipeline.yaml"
spec:
  schedule: "0 3 * * *"
  sourceDeletes:
    mode: snapshot
    misses: 2        # complete runs without the record before it goes
    maxShare: 10     # at most 10 % of the records per run; more holds the run
```

How you can be sure a removal is real:

- A run that failed, timed out or read fewer records than the source says it holds deletes nothing.
- A record goes only after it is missing from `misses` complete runs in a row.
- If one run would remove more than `maxShare` percent of the records, nothing is removed: the run is held and you get an alert. Check the source, then release the run.
- Only records this pipeline wrote are removed. Fields another pipeline wrote into the same entity stay, and a record a person edited is listed in the run instead of removed.
- The dry run of the pipeline lists what the next run would remove, before anything goes.
- The pipeline needs a Policy that lets it delete those types; the Portal refuses the pipeline until one does.

A removed record stays in the history with the time it was deleted and the reason `source-snapshot` or `source-event`, so you can always see when and why it left. Choose `snapshot` instead of pipeline expiry when the source sends its whole list: expiry guesses from the age of the last update, a snapshot knows. The two cannot be set on one pipeline.

## Related

- [Pipelines & Ingestion](./04-pipelines.md): building the pipeline, its **Lifetime** and its mapping.
- [Organizations, Projects & Context Spaces](./02-organizations-projects-spaces.md): creating the space whose rules this page sets.
- [Data Modeling](./03-data-models.md): the types a retention rule names, and personal attributes.
- [Users, Roles & Approvals](./07-users-roles-approvals.md): approving a retention or expiry change.
- [Export, Portability & Project Duplication](./09-export-import.md): what the Import page imports.
