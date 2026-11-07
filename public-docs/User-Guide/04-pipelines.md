---
sidebar_position: 5
title: Pipelines & Ingestion
description: Deploying, monitoring, and testing data ingestion pipelines, connecting data sources, and instantiating blueprints.
routes:
  - /projects/$project/pipelines
  - /projects/$project/datasources
  - /projects/$project/flows
  - /projects/$project/blueprints
  - /projects/$project/mqttbrokers
---

# Pipelines & Ingestion

A pipeline reads a data source and writes entities into a context space. This guide explains how to declare a data source, build the pipeline that reads it, test the mapping on a sample before anything is proposed, set up a whole feed from a ready-made set-up, and see whether a pipeline is running. It is written for data stewards, integration engineers and pipeline operators.

Every pipeline change follows the change approval flow: you configure the source or pipeline, test the mapping or probe the feed, click **Propose change**, and an approver reviews and merges the proposal in Approvals before execution begins.

## 1. Execution Classes and Change Flow

A pipeline runs in one of two ways, and **Execution** decides which:

- `resident` runs continuously: a feed that pushes (an MQTT broker, a WebSocket) or a **Period**
  shorter than 30 seconds.
- `scheduled` runs and finishes: a **Period** of 30 seconds or more, or a **Schedule** written as
  five cron fields such as `10 0 * * *`. Between runs it costs nothing.
- `auto` is the third choice and the easiest: the platform reads the period and picks `resident`
  under 30 seconds, `scheduled` at 30 seconds and above.

## 2. Connecting an External Data Source

### Add a Data Source Connection

#### By hand

1. Navigate to **Data sources** at `/projects/helsinki/datasources`.
2. In **Type**, choose the connection type (such as `HTTP endpoint` or `MQTT broker`).
3. Click **New data source**.
4. In **Name**, enter `hsl-citybikes-free`.
5. In **URL**, enter `https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/free_bike_status.json`.
6. Set **Timeout** to `15s`. If authentication is required, select credential references in **Credentials** (secret references take a secret name and key; raw values are never stored).
7. Click **Check**. You should see **Planned change** and the probe results displaying fetched records and byte counts.
8. Click **Propose change**.
9. In `/projects/helsinki/approvals`, an approver clicks **Approve**.

The live journey `load.spec.ts` replays these steps.

#### By asking the assistant

Type into the assistant composer:
`Add an HTTP data source called hsl-citybikes-free that reads https://gbfs.theta.fifteen.eu/gbfs/2.2/helsinki/en/free_bike_status.json`

The assistant opens the creation dialog on `/projects/helsinki/datasources` with the URL pre-filled. You click **Check**, verify the probe report, and click **Propose change**. An approver merges the proposal.

The live journey `assistant-creates.spec.ts` replays these steps.

## 3. Building and Testing Pipelines in the Studio

### Build and Test an Ingestion Pipeline

#### By hand

1. Navigate to **Pipelines** at `/projects/helsinki/pipelines`.
2. Click **New pipeline**.
3. Under **1. Source**, choose **Read from** as `A data source of this project (external feed)` and pick `hsl-citybikes-free`.
4. In **Pipeline**, enter `citybikes-free` and set **Period** to `60s`.
5. Set **Target endpoint** to `helsinki-all`.
6. In **Compute**, choose **Kind** as `Bloblang mapping`.
7. Enter the Bloblang mapping expression that maps input records to NGSI-LD entities, or drop a sample file to auto-draft the mapping.
8. In **Output**, enter **Entity type** as `Vehicle` and set **Write mode** to `upsert`.
9. Under **Try it on a sample**, click **Test on the source's URL** and click **Test mapping**.
10. You should see **All messages map to entities**, and **Propose change** becomes enabled.
11. Click **Propose change**.
12. In `/projects/helsinki/approvals`, an approver merges the proposal.

The live journey `load.spec.ts` replays these steps.

#### By asking the assistant

Type into the assistant composer:
`Create a pipeline called citybikes-free that reads the hsl-citybikes-free data source every 60s and writes into the helsinki space through the helsinki-all endpoint`

The assistant opens the studio dialog at `/projects/helsinki/pipelines` with the parameters configured. You verify the mapping test and click **Propose change**.

The live journey `assistant-creates.spec.ts` replays these steps.

## 4. Calculating KPIs with Pipeline Presets

### Configure an Indicator Pipeline

#### By hand

1. In `/projects/helsinki/pipelines`, click **New pipeline**.
2. In **Preset**, select `Analyse (KPI)`.
3. In **Source endpoint**, select `helsinki-all`.
4. Set **Indicator name** to `bikes-available-avg`.
5. Set **Entity type** to `BikeHireDockingStation` and **Attribute** to `availableBikeNumber`.
6. In **Aggregate**, select `Average`.
7. In **Period**, enter `15m`.
8. Click **Test on the endpoint's page**. You should see **Computed value** calculate and display a numeric result.
9. Click **Propose change**.
10. In `/projects/helsinki/approvals`, an approver reviews and merges the proposal.

The live journey `analyse.spec.ts` replays these steps.

#### By asking the assistant

Type into the assistant composer:
`Create a KPI pipeline called bikes-available-avg calculating average availableBikeNumber every 15m from endpoint helsinki-all`

The assistant populates the KPI preset in the studio, compiles the indicator formula, and computes the preview value. You click **Propose change**, and an approver merges the change.

## 5. Deploying Flows from Blueprints

### Instantiate a Blueprint

#### By hand

1. Navigate to **Data sources** at `/projects/helsinki/datasources`.
2. Under **Ready-made set-ups**, read the cards: a GBFS city-bike feed, a JSON feed polled into a Context Space, an MQTT feed. Each shows its review lane (**Approved automatically**, **One approver**, or **Full approval**). The section lists only the set-ups your role may run; with none, it is absent.
3. Click **Set up** on a card.
4. Fill in the parameters generated from the blueprint schema (such as the feed address, the poll period and the Endpoint the flow writes through).
5. Click **Propose change**. **Back to data sources** leaves without writing anything.
6. The platform expands the blueprint into manifests and opens a change proposal.
7. In `/projects/helsinki/approvals`, an approver approves the proposal.

An application from a prompt starts from **Applications** with **New application**. The old address `/projects/helsinki/flows` opens the project's home.

#### By asking the assistant

The assistant can recommend blueprints matching your data source and start one with the `jc_flow_start` tool, but parameter forms must be submitted and reviewed by you.

## 6. Pausing, Resuming, and Stream Health

### Manage Live Pipelines

#### By hand

1. On `/projects/helsinki/pipelines`, inspect live operational numbers in the table: message throughput rate (**msg/s**), received messages, error counts, and p99 latency.
2. In the row actions menu, click **Pause** on a running pipeline.
3. This creates a proposal setting `spec.enabled: false`.
4. In `/projects/helsinki/approvals`, an approver clicks **Approve**. The stream halts cleanly.
5. To restart the stream, click **Resume** on the paused row and approve the resulting proposal.

The live journey `change.spec.ts` replays these steps.

#### By asking the assistant

Type into the assistant composer:
`Pause the hel-news pipeline`

The assistant opens `/projects/helsinki/pipelines?edit=hel-news` with the form toggled to pause the pipeline. You click **Propose change**, and an approver confirms it in Approvals.

The live journey `change.spec.ts` replays these steps.

## 7. Removing Entities the Source Dropped

A pipeline writes what its source sends and never deletes. When a feed stops listing a vehicle, the vehicle stays in the space with its last position. If the source sends the whole picture every run, you can tell the pipeline that absence means removal.

Use it for full-snapshot sources: a vehicle list, a register of stations, a catalogue export. An entry missing from the snapshot is gone.

Leave it off for incremental and event feeds: sensor readings, change streams, a feed that only sends what changed. There a quiet entity has nothing new to say, and expiry would delete it. That is why expiry is off unless you switch it on.

When the source gives each item an end time, give the pipeline a **Lifetime** instead (section 8); [Retention & Expiry](./14-retention-and-expiry.md#3-which-one-do-i-pick) compares the ways data is removed. A pipeline cannot carry both expiry and a whole-entity lifetime for the same type.

### Switch On Expiry

#### By hand

1. On `/projects/helsinki/pipelines`, open the pipeline and switch on **Remove entities the source no longer sends**.
2. Enter the window (for example `14d`) and the entity types this pipeline writes.
3. Click **Propose change**. The Portal refuses the proposal while another pipeline writes into the same space, and while no Policy grants the pipeline's account `deleteBatch` on those types; the message names the Policy to extend.
4. An approver confirms it in Approvals. The pipeline page and the list then say "Entities not updated for 14 days are removed", and the deletions appear in the activity trail under the pipeline's account.

## 8. Ending Entities and Fields With a Lifetime

A lifetime gives what the pipeline writes an end time, and the broker removes it when that time passes. Unlike expiry it needs no delete permission and no second sweep: the end time travels with each write.

### Set a Lifetime

#### By hand

1. On `/projects/helsinki/pipelines`, open the pipeline and click **Edit pipeline**.
2. Open the **Lifetime** step. Choose one of the four:
   - **Whole entity expires**, **From a field**: pick the **Field** that holds the time, for a bus `observedAt`, and the margin in **Plus**, for a bus `5` `minutes`. For an alert pick `validTo` and set **At most after the write** to `30 days`, so a wrong year does not keep it for decades.
   - **Whole entity expires**, **After each write**: the entity is removed the given time after the pipeline last wrote it.
   - **Whole entity expires**, **On a fixed date**: every entity of the pipeline ends at one date and time, for an event that closes on a known day.
   - **Only some fields expire**: pick the fields, for a bus `location`, and how long each stays, for example `5` `minutes` **After each write**. The entity stays; only those fields are removed.
3. Click **Check**. Each sample record is checked against the space's data model, what the target endpoint lets the pipeline write, and the lifetime it gets; a problem names the record, the field and the rule.
4. When the check is green, click **Propose change**. The Portal refuses a proposal whose check is missing or older than the draft, a fixed date that has passed, and a mapping that sets `expiresAt` for the same entity or field.
5. An approver merges the proposal in Approvals. The editor reads the lifetime back from the manifest, even after you edit the mapping by hand.

The four choices and when each fits are in [Retention & Expiry](./14-retention-and-expiry.md#4-the-lifetime-step-of-the-pipeline-editor).

## 9. Running an MQTT Broker for Your Devices

When devices push their readings over MQTT, the project can run its own broker. **MQTT brokers**, at `/projects/{project}/mqttbrokers`, lists them; a pipeline then reads the broker's topics like any other source.

1. Click **New MQTT broker** and give it a **Name**.
2. Under **Roles**, add one role per group of devices: its **Service accounts (devices)**, the topics it **May publish to** such as `sensors/+/air`, and the topics it **May subscribe to**.
3. Choose **What the broker does**: **Ingest** (a pipeline reads each message), **Ingest and keep** (the broker also keeps the last messages under **Kept messages**) or **Two-way** (devices also receive messages).
4. Set the **Limits**: **Largest message (bytes)**, **Messages per second per device** and **Connections at most**. To admit devices only from your own networks, list them under **Allowed source networks**, one CIDR range per row (**Add** gives another) such as `203.0.113.0/24`; left empty, any address may connect with a valid token.
5. Click **Propose change**.

Open the broker once it is approved. **Topic access** shows which role may publish and subscribe where; the connect panel gives the **Address**, the **Client id and user name**, how to get the **Password** and **Certificate**, and a **Try it** command to send a test message; **Measured** shows how many devices are **Connected now**; **Connections** draws who talks to whom and marks a step that needs **Fix it**.

When the page says **MQTT is off for this project**, an organization administrator turns it on under **Administration**, in the project's MQTT settings. The same administrator raises the number of brokers a project may have.

### Giving devices a broker role

The broker says what each role may do; a device's service account says that the device holds the role. To admit a fleet of air sensors to the role `air-sensor` of the broker `air`, add one binding to the fleet's service account and propose the change:

```yaml
# spec.roles of the ServiceAccount air-fleet-north (excerpt)
- role: mqtt/air/air-sensor     # mqtt/*/air-sensor: every broker of the project with that role
  scope:
    project: bratislava         # always the account's own project
  validity:
    notAfter: "2027-03-31T23:59:59Z"
```

From the approval on, each device of `air-fleet-north` may publish where `air-sensor` allows, until the end of March 2027. Removing the binding, or letting `notAfter` pass, stops the device at its next publish; the broker manifest stays as it was. A broker that still lists `serviceAccounts` on a role keeps working for now, and `jcctl migrate` moves those names into bindings like the one above.

## Related

- [Organizations, Projects & Context Spaces](./02-organizations-projects-spaces.md): managing target spaces.
- [Data Modeling](./03-data-models.md): authoring schemas for pipeline validation.
- [Endpoints & Sharing](./05-endpoints-and-sharing.md): publishing ingested context data.
- [Users, Roles & Approvals](./07-users-roles-approvals.md): reviewing pipeline change proposals.
- [Retention & Expiry](./14-retention-and-expiry.md): the lifetime recipes for buses, alerts, events and single fields, and when to pick expiry instead.
