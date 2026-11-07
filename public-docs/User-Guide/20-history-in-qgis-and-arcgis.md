---
sidebar_position: 22
title: Show History in QGIS and ArcGIS
description: Load the stored history of an Endpoint's data into QGIS or ArcGIS Pro and play it on a time slider.
surfaces: ogc=1-the-history-layers ogc=2-qgis ogc=3-arcgis-pro ogc=4-what-the-history-holds
---

# Show History in QGIS and ArcGIS

An Endpoint that serves OGC API - Features and lets you read history offers two layers per data type: the type itself, with the current values, and the same type with `-history` after its name, with every stored state. This page shows how to load a history layer into QGIS or ArcGIS Pro and play it on the time slider, for example to watch the vehicles of a fleet move through a morning or the free places of the bike stations change over a day. You need QGIS 3.28 or newer, or ArcGIS Pro 3.1 or newer, and the Endpoint's address from its page in the Portal.

## 1. The History Layers

The address of the OGC API - Features service of an Endpoint is

`https://api.{domain}/api/endpoint/{slug}/ogc/features`

`{slug}` is the Endpoint's identifier from its page in the Portal, `{domain}` the domain of the platform instance (`dev.joinedcontext.com` on the demonstration instance). The list of collections names each data type twice: `Vehicle` holds each vehicle once, where it is now; `Vehicle-history` holds one feature for each time a vehicle reported its values. The history layer shows only what the Endpoint lets you see in the current layer: a value the Endpoint hides is hidden in its history too.

Each feature of a history layer has the values valid at one moment, and three fields of its own:

| Field | What it holds | Example |
|---|---|---|
| `entityId` | the identifier of the thing the state belongs to | `urn:ngsi-ld:Vehicle:HSL:1234` |
| `validFrom` | when these values were observed | `2026-10-21T07:30:00Z` |
| `validTo` | when the next values were observed; empty for the newest state | `2026-10-21T07:35:00Z` |

The place of a feature is where the thing was at `validFrom`. A thing whose place never changes, such as a bike station, keeps its place in every state.

## 2. QGIS

1. Open Layer, Add Layer, Add WFS / OGC API - Features Layer.
2. Choose New, give the connection a name, paste the Endpoint's address from section 1 into URL and set Version to OGC API - Features. When the Endpoint asks for a sign-in, add your credentials under Authentication. Select OK.
3. Select Connect, choose the history layer, for example `Vehicle-history`, and select Add.
4. Open the layer's Properties, go to Temporal, tick Dynamic Temporal Control and choose Separate Fields for Start and End Date/Time. Set Start field to `validFrom` and End field to `validTo`. Select OK.
5. Open View, Panels, Temporal Controller. Select Animated Temporal Navigation, set the range to the hours you want to watch (the button Set to Full Range takes the whole history) and a step such as 5 minutes, and press play.

QGIS draws each state while its time range holds the current frame. The newest state of each thing has no `validTo`, so QGIS keeps it on the map to the end of the animation.

To load less data, filter the layer before you add it: in the Add Layer dialog, Build Query takes a condition on the fields, for example `validFrom > '2026-10-21T06:00:00Z'`.

## 3. ArcGIS Pro

1. On the Insert tab, open Connections, Server, New OGC API Server, paste the Endpoint's address from section 1 and select OK. When the Endpoint asks for a sign-in, give your credentials in the same dialog.
2. In the Catalog pane, open Servers, the new connection, and drag the history layer, for example `Vehicle-history`, onto the map.
3. Open the layer's Properties and go to Time. Set Layer Time to Each feature has a start and end time field, Start Time Field to `validFrom` and End Time Field to `validTo`. Select OK.
4. The Time slider appears at the top of the map. Set its span, for example 5 minutes, and press play.

When ArcGIS Pro lists `validFrom` as a text field and offers no time setting, export the layer to a feature class in your project and run the Convert Time Field tool on `validFrom` and `validTo` with the format `yyyy-MM-ddTHH:mm:ssZ`, then set the time of the new fields as in step 3.

## 4. What the History Holds

- A history holds what the data space keeps: a data type whose space keeps no history has an empty history layer. The retention of each type is set on the space's Retention tab (see [Retention & Expiry](14-retention-and-expiry.md)).
- Each thing shows its newest 1,000 reports per value. An older time range shows no features.
- A history layer is read only: QGIS and ArcGIS Pro offer no editing on it.
- A page of the history ends after 100 things even when it holds fewer features than asked; QGIS and ArcGIS Pro follow the link to the next page by themselves.

## Related

- [Endpoints, Data Sharing & Integration](05-endpoints-and-sharing.md)
- [Retention & Expiry](14-retention-and-expiry.md)
- [Endpoint representations](../API/02-endpoint-representations.md)
