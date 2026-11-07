---
sidebar_position: 13
title: Event Categories
description: The one list of 16 main event categories every city's events carry in `category`, their labels in four languages, the Smart Data Models codes they map to, and the rule table that picks one from a source's own words, and the decision model that names the rest.
---

# Event Categories

Every Event in every city carries one main category from the list below in its `category` attribute, so a person filters "sport" across Banská Bystrica, Helsinki, Praha and Barcelona with one query. This page is the vocabulary: the codes, their labels, the Smart Data Models values behind them, the rule table that maps a source's own words to a code, and the decision model that names the events no rule decides. Model authors, pipeline authors and anyone who filters events read it. Changing the list is a change to this page first, then to `seeds/event-categories.json` in the deployment repository, which every pipeline and model reads (T-5363).

## 1. The list

The data holds the code; the Portal, the catalogue, the map and the assistant show the label in the reader's language. Each model that holds Events declares the codes as the enum `EventCategory` with these four labels, and a test fails when a model and `seeds/event-categories.json` disagree.

| # | Code | sk | cs | en | de | Smart Data Models `category` |
|---|---|---|---|---|---|---|
| 1 | `sport` | Šport | Sport | Sport | Sport | none (free value) |
| 2 | `outdoor-nature` | Príroda a turistika | Příroda a turistika | Outdoors and nature | Natur und Wandern | `outdoorActivities`, `excursion` |
| 3 | `film` | Film | Film | Film | Film | `spectacle` |
| 4 | `theatre-dance` | Divadlo a tanec | Divadlo a tanec | Theatre and dance | Theater und Tanz | `spectacle` |
| 5 | `music` | Hudba | Hudba | Music | Musik | `spectacle` |
| 6 | `festival-fair` | Festivaly a jarmoky | Festivaly a jarmarky | Festivals and fairs | Feste und Jahrmärkte | `spectacle` |
| 7 | `food-market` | Gastronómia a trhy | Gastronomie a trhy | Food and markets | Essen und Märkte | `gastronomy`, `shopping` |
| 8 | `exhibition-museum` | Výstavy a múzeá | Výstavy a muzea | Exhibitions and museums | Ausstellungen und Museen | `museum` |
| 9 | `history-heritage` | História a pamiatky | Historie a památky | History and heritage | Geschichte und Denkmäler | `history` |
| 10 | `faith-tradition` | Viera a tradície | Víra a tradice | Faith and traditions | Glaube und Brauchtum | `religiousWorship` |
| 11 | `education-talk` | Vzdelávanie a prednášky | Vzdělávání a přednášky | Education and talks | Bildung und Vorträge | none (free value) |
| 12 | `children-family` | Deti a rodina | Děti a rodina | Children and family | Kinder und Familie | none (free value) |
| 13 | `hobby-workshop` | Hobby a tvorivé dielne | Hobby a tvořivé dílny | Hobbies and workshops | Hobby und Workshops | none (free value) |
| 14 | `health-wellness` | Zdravie a wellness | Zdraví a wellness | Health and wellness | Gesundheit und Wellness | `wellness` |
| 15 | `community-civic` | Komunita a samospráva | Komunita a samospráva | Community and civic life | Gemeinschaft und Bürgerleben | none (free value) |
| 16 | `other` | Iné | Jiné | Other | Sonstiges | none (free value) |

`other` has no rule: only the decision model (section 4) gives it, to an event no other code fits.

The model is Smart Data Models' `dataModel.TourismDestinations/Event`, the only official SDM Event. Its suggested `category` values (excursion, gastronomy, history, museum, outdoorActivities, parksAndGardens, religiousWorship, shopping, spectacle, wellness) lack sport, education and community life, and SDM allows free values, so the platform's 16 codes are the values; the last column names the SDM value each one corresponds to.

## 2. The attributes

| Attribute | Value | Example |
|---|---|---|
| `category` | A list of one code from section 1, as SDM's Event writes `category`. Absent while neither a rule nor the decision model has named the event. | `["music"]` |
| `categorySource` | How the code was picked: `rule` (section 3) or `model` (section 4). Present exactly when `category` is. | `rule` |
| `audience` | SDM's Event audience codes, set with `children-family`: `children` when the source's word or the name says children, else `family`. | `["children"]` |
| `eventCategory` | The source's own word, kept unchanged so a category can be traced back to it. | `hudba (koncert)` |

A filter is the standard NGSI-LD query, `q=category=="sport"`, on every Endpoint that serves Events; a list value matches when one of its members does.

## 3. The rule table

The pipeline picks the code once per run, when it writes the event, never per read. The first match wins, in this order:

1. The source's own word, without its audience part (`akcie pre deti`) and without the catch-all family `trhy, výstavy, veľtrhy`, whose `(ostatné)` member counts as a market.
2. The event's name.
3. The source's whole word, so `ostatné (akcie pre deti)` still gives `children-family`.
4. The kind of body whose calendar it is, for a body that announces one kind only (a theatre's plays, a museum's exhibitions), from the `org_type` column of the source inventory.

Inside each step the categories are tried in the order of the table below, so `hudba (tanečná zábava)` is music and `kino, pre deti` is film. A text no rule matches leaves `category` to the decision model (section 4); a rule never writes the catch-all `other`.

| Code | Word stems the rule matches (lower case, RE2) | Kinds of body whose calendar holds only this |
|---|---|---|
| `sport` | šport, sport, futbal, hokej, volejbal, nohejbal, basketbal, hádzan, tenis, kolky, karate, beh (at a word start: príbeh is no run), maratón, turnaj, zápas, pohár, cyklo, korčuľ, plávan, lyžov, šach, atletik, fitnes, zumba, bowling, biliard, súťaž, hasič, krampuslauf, run, fk, mfk, bežme, bežíme, regat |  |
| `outdoor-nature` | turistik, túr (a word start, so kultúra is no hike), výstup, výlet, príro, vychádzk, pochod, hike, nature, outdoor, jaskyň, splav, rastlín, stromy, záhrad, vtác, zviera, chodník |  |
| `film` | kino, film, premietan, projekci, cinema, movie | cinema |
| `theatre-dance` | divad, činohr, opera, balet, tanec, muzikál, predstaven, inscenác, bábk, repríza, premiér, kabaret, stand-?up, komédi, kúzel, zájazd, theat, dance, sľuk, lúčnic, színház, činoher | theatre, folk ensemble |
| `music` | hudb, hudob, koncert, music, spev, zbor, orchest, kapel, band, trio, kvartet, recitál, jazz, rock, disco, disko, párty, party, karaoke, dj, heligón, dychov, cimbal, organ, folklór, zábava, veselic, tančiare, ples (not pleso), bál, vivaldi, zenekar, jam session, feat., ľudová nôta, ľudové piesne, klavír |  |
| `festival-fair` | festival, fest, jarmo, hody, hodov, dni obce, dni mesta, slávnos, oslav, karneval, maškar, fašiang, majáles, dožink, vinobran, ohňostroj, vatr, silvest, lampión, halloween, strašidel, tekvič, mikuláš | festival |
| `food-market` | gastro, trh, jedl, ochutnávk, degust, vín, vinár, pivo, guláš, kapustnic, zabíjač, varen, pečen, burz, bazár, punč, kuchár, kuchyn, palacink, coffee, kávy, food, market, swap, špecialít |  |
| `exhibition-museum` | výstav, vernisáž, finisáž, kurátor, galéri, múze, muze, expozíc, exhibit, museum, gallery, maľb, insit, sochár, ateliér, salón, architekt, expo, výtvarn | museum, gallery |
| `history-heritage` | histor, pamiatk, hrad, kaštie, zámo, dejin, výroči, spomienk, piet, vencov, vojn, veterán, snp, remesl, kódex, prehliadk, baníc, patrón, minulos, dedičstv, histór, zámk, baník, povstan |  |
| `faith-tradition` | cirkev, omš, bohoslužb, kostol, púť, svät, modlitb, duchovn, poklon, pobožn, evanjel, reformač, farsk, farnos, biblick, advent, vianoc, vianoč, veľkonoč, dušičk, svetlono, koled, koľad, tradíci, diecéz, kroj |  |
| `education-talk` | prednášk, seminár, konferenc, besed, workshop, kurz, vzdeláv, diskus, čítan, autorsk, knih, kniž, literár, poézi, poeti, básn, vedeck, ved, webinár, info dni, študent, talk, lecture, ai, haiku, poviedk, recenzi, kvíz, inteligenc, kvíz, kviz, cestovateľ, planetári, hvezdár, filozof, dialóg, prezentáci | library, faculty, university |
| `children-family` | deti, detsk, pre najmenš, rozprávk, rodin, rodič, škôlk, mamičk, story time, kids, children, family |  |
| `hobby-workshop` | tvoriv, tvoren, tvoríme, dielni, ručné práce, ručných prác, háčkov, pleten, plsten, modrotlač, ikeban, vyrezáv, spoločensk[ýé]ch hier, stolov[áé] hr, chovateľ, fotograf, hobby, craft, potlač, čipk, 3d tlač |  |
| `health-wellness` | zdrav, wellness, cvičen, pohyb, kvapka krvi, darovanie krvi, meditác, relax, somatick, masáž, medicín, jóg, joga, parkinson, ženské kruhy, open class, psych |  |
| `community-civic` | komunit, stretnut, posedeni, zhromažd, fórum, forum, občian, dobrovoľn, brigád, spolok, klub, schôdz, ocenen, uvítan, jubil, prípitok, charit, zbierk, slobod, seniorov, susedia, sused |  |

Measured on dev on 2026-10-22 over the 954 events of `slovensko-podujatia`: 739 remain after the notice filter, and the table gives 653 of them (88.4 %) a category. A hand read of 35 categorised events found 2 debatable codes. On 2026-10-24 the table with T-5490's stems decided 170 of 236 recorded events (72 %: Banská Bystrica's city calendar on dev and the pipeline fixtures, whose source words it cannot read), the rest going to section 4. T-5490 also fixed three stems that matched inside other words: `túra` in `kultúra`, `beh` in `príbeh`, `ples` in `pleso`.

### Source families

| Family | Words read | State |
|---|---|---|
| Slovak town, village and culture-house calendars (`seeds/bbsk-events`, 302 pipelines) | the calendar's category (`hudba (koncert)`, `Kino, Pre deti`, `cirkevný (ostatné)`), the name, the inventory's `org_type` | mapped |
| Helsinki Linked Events | YSO keywords | not yet mapped |
| Praha, Barcelona agenda feeds | the agenda's own categories | not yet mapped |

## 4. The decision model

An event no rule names gets its code from a decision model, run offline by `scripts/events-classify.py` in the deployment repository, never inside a cluster pipeline (owner 2026-10-24, T-5490):

1. The script reads a store's events through its public NGSI-LD API and applies the rule table of section 3 itself.
2. For each event no rule decides it sends the model only public event text: the name, the source's own word, the first 500 characters of the description and the venue.
3. The model answers through a JSON schema whose only member is `category`, an enum of the 16 codes. The script checks the answer against the list again; anything else, a URL or a word off the list, becomes `other`. An injected instruction in an event's text can only give a wrong code.
4. The answer goes into `seeds/bbsk-events/categories.csv`, keyed by the source and the first 16 hex digits of SHA-256 over the lower-cased name and venue (`name|venue`). The pipeline generator bakes each source's rows into its pipeline, which computes the same key and writes `categorySource: model`. An event already in the table is never sent again.

The OpenRouter key is read at run time from the secrets mount and never printed or committed; each call has a 30 s timeout and a 64 KiB answer cap, and `--budget` (default 1 USD) stops the run before the next call once spent. An event that appears between two runs carries no `category` until the next run.

### Model choice

The model is the cheapest of the candidates that agrees with a hand-labelled sample of 100 events (`seeds/bbsk-events/category-sample.csv`, labelled 2026-10-24 from the name, the source's word, the description and the venue) on at least 90 of them. Measured 2026-10-24:

| Model | Agreement | Cost per 1,000 events | Chosen |
|---|---|---|---|
| `deepseek/deepseek-v4.1-flash` (reasoning off) | 88 % | 0.045 USD | no, under 90 % |
| `google/gemini-3.8-flash` (reasoning low, which it requires) | 91 % | 0.69 USD | yes |

The misses of both are events whose text does not say what they are (`Mboty`, `Jojko TOUR 2026`) and the boundary between talks, workshops and children's programmes. Gemini answers in about 15 s per call, so a run over 1,000 undecided events takes about four hours.

## Related

- [Architecture: Data Models](../Architecture/11-data-models.md), how a model's enum and labels reach every surface.
- [API/02 Endpoint Representations](02-endpoint-representations.md), the surfaces that show or filter `category`.
- [API/11 Mapping and Representation Contract](11-representation-contract.md), where an Endpoint names the members a format shows.
- [Requirements: Data Models](../Requirements/data-models.md), the model rules the Event models follow.
