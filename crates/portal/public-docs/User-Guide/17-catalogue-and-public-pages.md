---
sidebar_position: 19
title: The Catalogue and Public Pages
description: What a visitor without an account finds in the Portal, how to search the catalogue and read one dataset, and how a project publishes a dataset, a public view, a public form and an AI connector.
routes:
  - /catalogue
  - /catalogue/$name
  - /catalogue/endpoint/$slug
  - /v/$slug
  - /d/$slug
  - /f/$slug
  - /connect/$project/$name
  - /glossary
  - /accessibility
---

# The Catalogue and Public Pages

Some pages of the Portal open without signing in: the catalogue of public datasets, a dataset's own page, a published view, a public form, an Endpoint's API documentation, the page that connects an AI assistant, the glossary and the accessibility statement. This page explains each of them for two readers: the visitor who uses them, and the data steward who publishes what they show.

## 1. Search the Catalogue

Open `/catalogue`. **Catalogue** lists every public dataset of this installation's open-data catalogues.

1. Type a few words into **Search datasets**, such as `air quality`, and press **Search**. The search reads titles, descriptions and keywords.
2. Narrow the list under **Filters**: **Publisher**, **Theme**, **Format**, **Licence**, **Area** and **Period**. On a phone the filters open as a panel; **Close filters** returns to the list.
3. Page through the results with **Previous page** and **Next page**, and open a dataset by its title.
4. **Clear search and filters** starts again.

The search and the filters live in the address bar, so you can send a filtered list to someone as a link.

When the list is empty:

- **No public dataset yet**: no project of this installation has published a dataset. A dataset appears once a project publishes an Endpoint to its open-data catalogue (section 3).
- **No dataset matches**: remove a filter or change the search text.
- A notice naming catalogues that did not answer means their datasets are missing from this list for now. Try again in a minute.

## 2. Read One Dataset and Use Its Data

A dataset's page, `/catalogue/{name}`, holds everything you need before you use the data.

- The facts at the top say who publishes it (**Publisher**), under which **Licence**, how often it is updated, which **Area** and **Period** it covers, whom to contact and when it last changed.
- **Resources** lists the files and services of the dataset, each with **Download** or **Preview**.
- **Sample** reads ten rows through the Endpoint as you open the page, so you see real values, not a description of them.
- **Data model** names the model the rows follow, with a link to **Model documentation**.
- **Use this data** gives ready-made snippets: **From a shell (NGSI-LD)**, **Download as CSV**, **In an App built on this endpoint** and **In an MCP client**. Each has a copy button.
- **Open in the catalogue** opens the same dataset in the open-data catalogue itself, and **All datasets** returns to the list.

If the page says **No public dataset has this name**, the dataset was removed or made private. Search the catalogue for it.

## 3. Publish a Dataset to the Catalogue

You need a project in which you may propose an Endpoint. Then **Publish a dataset** appears on the catalogue page.

1. Open `/catalogue` and click **Publish a dataset**.
2. Under **1. Endpoint**, choose the Endpoint whose data the dataset describes.
3. Under **2. Description**, fill in **Dataset title**, **Description**, **Contact name**, **Contact e-mail** and **Keywords**, and the period under **Covers data from** and **Covers data until**. **Draft the description** fills the text from the Endpoint's model; read and correct it before you go on.
4. Click **Preview the entry**. **3. Preview** shows the entry as visitors will see it.
5. Click **Propose publication**. When the Endpoint is not public yet, the button reads **Make public and propose publication**, and you type the Endpoint's name to confirm, because a public Endpoint can be read by anyone.

The dataset appears in the catalogue after an approver approves the change. Publishing to a national portal such as data.slovensko.sk is described in [National Open Data Portals](13-national-open-data-portals.md).

## 4. Publish a Public View, a Public Form and API Documentation

These three public addresses are created from a Context Space or an Endpoint inside a project; a visitor opens them without an account.

### A public view, `/v/{slug}`

In a Context Space, **Publish this type as a public link** publishes a read-only table of one type, with the attributes you leave ticked. A visitor sees the table, switches between types under **Types** when the view has several, and cannot change anything. A view that was withdrawn shows **This view is not published, or not any more.**

### A public form, `/f/{slug}`

**Publish as a public form** lets anyone create one record in a space, for example a report of a broken street lamp. You choose the fields the form asks for, how many **Entries a day** it accepts, and you can **Embed on a website**. **Propose the public form** sends it to an approver; **Send a test** tries it before.

A visitor fills in the fields and presses **Create**. A field marked as required must be answered, and the form says which answer needs fixing, for example "Enter a whole number, such as 12." The form tells the visitor that their answers become public data of the space and must hold no personal details.

### API documentation, `/d/{slug}`

A public Endpoint has its own **API documentation** page, generated from the Endpoint itself: what it answers, an example call for each read under **Request with curl**, the parameters of each address and a link to **The OpenAPI document (JSON)**. Send this address to a developer who wants to read your data; [Endpoints, Data Sharing & Integration](05-endpoints-and-sharing.md) explains how the Endpoint is published.

## 5. Connect an AI Assistant to Public Data

A public MCP server of a project has a page of its own at `/connect/{project}/{name}`, titled **Connect an AI assistant**. It needs no account and no key.

1. Copy the **Connector address** with **Copy the address**.
2. Under **Set up your client**, follow the section of your client: Claude Code (**Copy the command**), Claude Desktop (**Copy the configuration**), ChatGPT (steps to follow) or **Other MCP clients**.
3. Ask the assistant the question the page suggests, such as "What data does this offer? Show me five records with a link to the source."

The page says how many tools the server offers, that it is read-only, and how many requests a minute one network may send. When it says **Too many requests from your network right now**, wait the seconds it names. Signed-in people build their own connectors on **My MCP connectors**, described in [Working with AI Agents](08-working-with-ai-agents.md).

## 6. Glossary and Accessibility Statement

- **Glossary**, at `/glossary`, explains each term the platform uses in one sentence with one example. A term hint elsewhere in the Portal links to its entry.
- **Accessibility statement**, at `/accessibility`, is the statement a public body publishes: the legal basis, the compliance status, the content that is not accessible yet, how it was checked, and whom to write to. Every public page links it in its footer under **Accessibility**.

## Related

- [Endpoints, Data Sharing & Integration](05-endpoints-and-sharing.md) — how the Endpoint behind a dataset is made public.
- [National Open Data Portals](13-national-open-data-portals.md) — registering the catalogue with data.slovensko.sk and data.gov.cz.
- [Export, Import & Synchronization](09-export-import.md) — publishing open data to CKAN.
- [Working with AI Agents](08-working-with-ai-agents.md) — personal MCP connectors for signed-in people.
