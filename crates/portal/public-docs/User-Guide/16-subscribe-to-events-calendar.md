---
sidebar_position: 18
title: Subscribe to Events in Your Calendar
description: Put the upcoming events of the Banská Bystrica Region, or of all of Slovakia, into Google Calendar, Outlook or Apple Calendar with one link that keeps itself up to date.
---

# Subscribe to Events in Your Calendar

The platform collects the public events that towns, municipalities, regions and cultural bodies in Slovakia announce on their web sites, and publishes them as two datasets: the events of the Banská Bystrica Region, and the events of all of Slovakia. Each dataset has a calendar link in the iCalendar format (RFC 5545). You paste the link into your calendar application once; the application then fetches the link again on its own schedule, so new events appear and finished ones disappear without anything else to do. This page is for anyone who wants the events in a calendar: a resident, a tourist office, a school. You need no account.

## 1. The Two Calendars

| Calendar | What it holds | Link |
|---|---|---|
| Podujatia v Banskobystrickom kraji | the events of the Banská Bystrica Region | `https://api.{domain}/api/endpoint/hqgmj6ieuq57y37g5g4ckz5eyhebklox/ical/v1/events.ics` |
| Podujatia na Slovensku | the events of every collected source in Slovakia | `https://api.{domain}/api/endpoint/d2f5ngtn6hysvw5ytw45cgfkcpcricpz/ical/v1/events.ics` |

`{domain}` is the domain of the platform instance; on the demonstration instance it is `dev.joinedcontext.com`. The data catalogue page of each dataset shows the same link beside its other formats.

Each calendar entry has the event's name, its start and end, the venue's address, a map point and the link to the page where the organiser describes the event. An event the source publishes without an end shows its start only.

## 2. Add the Calendar

Use the subscription feature of your application, not its import: an import copies today's events once and never updates them.

| Application | Where |
|---|---|
| Google Calendar (web) | Other calendars, the plus sign, From URL; paste the link, then Add calendar |
| Outlook (web) | Add calendar, Subscribe from web; paste the link, give it a name, then Import |
| Apple Calendar (macOS) | File, New Calendar Subscription; paste the link, then Subscribe |
| iPhone and iPad | Settings, Calendar, Accounts, Add Account, Other, Add Subscribed Calendar |
| Thunderbird | New Calendar, On the Network; paste the link as the location |

Some applications accept the same link with `webcal://` in place of `https://`, which opens the application straight from a browser.

Each application chooses how often it fetches the link again: Google Calendar every few hours up to a day, Outlook and Apple Calendar according to their refresh setting. The datasets themselves are updated once a day, in the night.

## 3. Only Some Dates

Add `from` and `to` to the link to keep a calendar to a period, each a date (`2026-12-01`) or a date and time with its offset. `to` includes the whole day it names:

```text
https://api.{domain}/api/endpoint/hqgmj6ieuq57y37g5g4ckz5eyhebklox/ical/v1/events.ics?from=2026-12-01&to=2026-12-31
```

A link with `from` after `to`, or a value that is no date, answers an error instead of a calendar.

A calendar of one town or one category is not served as a link yet. To find the events of one town now, read the dataset's data with an NGSI-LD query, for example `q=address[addressLocality]=="Brezno"`, or filter the region's dashboard.

## 4. What Is in the Calendars

Only upcoming events are collected: a source's past events are never loaded, and an event leaves the dataset when it ends. Each source contributes at most its 100 soonest events per day. An event is published only when the source gives its name, its start and its place as a GPS point in Slovakia; the platform does not guess a place from the town's name. An event that two sources announce with the same name, start and town appears once.

The organiser of an event is responsible for its details. When an entry is wrong, the link in the entry leads to the organiser's own page; correct it there, and the calendar follows after the next daily update.

## 5. When the Calendar Stays Empty

| What you see | Why | What to do |
|---|---|---|
| The calendar is added and has no events | the sources have published no upcoming events with a GPS point, or the daily update has not run yet | check again the next day; the dataset page in the catalogue shows the number of events |
| The application says the link is invalid | the link was changed when pasted, or `webcal://` is not accepted | paste the `https://` link exactly as shown |
| Events you expect are missing | the source gave no GPS point, or the event is past its end | the organiser's page is the source; a missing point is the organiser's to add |

## Related

- [Endpoints & Sharing](./05-endpoints-and-sharing.md): what an Endpoint is and the formats it serves.
- [National Open Data Portals](./13-national-open-data-portals.md): where the datasets are catalogued.
- [Retention & Expiry](./14-retention-and-expiry.md): how an event leaves its space when it ends.
