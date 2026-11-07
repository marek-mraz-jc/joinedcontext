---
sidebar_position: 21
title: Data Spaces, Scopes and Environments
description: Joining a data space and offering data to its participants under an agreement, naming the scopes your data is filed under, and describing the installations a project's changes are promoted to.
routes:
  - /projects/$project/dataspaceparticipants
  - /projects/$project/dataoffers
  - /projects/$project/dataagreements
  - /projects/$project/scopedefinitions
  - /projects/$project/environments
---

# Data Spaces, Scopes and Environments

This page covers five kinds that most projects never need and a few need badly: the organization's membership of a data space, the data it offers there, the agreements it signs, the scopes it files data under, and the installations it promotes changes to. Each is a list with a form to add one; every save proposes a change that an approver approves. It is written for data stewards and platform administrators.

## 1. Join a Data Space

A data space is a group of organizations that exchange data under signed agreements, each through its own connector. **Data space**, at `/projects/{project}/dataspaceparticipants`, holds your organization's one membership; the button **New data space membership** appears only while there is none.

1. Click **New data space membership**.
2. Fill in the **Participant identifier (did:web)**, for example `did:web:data.example.sk`, the **Credential issuer** that vouches for you, the **Connector address** and the **Connector engine**.
3. Under **Trusted issuers**, add the issuers whose credentials you accept from others.
4. Click **Propose change**.

## 2. Offer Data and Sign an Agreement

**Data offers** say what you offer to the data space and on which terms.

1. Click **New data offer**. Choose the **Context space** and the **Offered endpoints**.
2. Write the terms under **Offer (ODRL, JSON)**, tick **Contains personal data** when it does, and state the **Purpose (DPV)**.
3. Click **Propose change**.

**Data agreements** record what one other participant may read under one offer. Click **New data agreement**, choose your **Role**, the **Data offer** and the **Other participant**, set the validity under **Valid from** and **Valid until**, and narrow what the agreement reads under **Query limits**: a **Query filter**, **Scope filter**, **Area filter** or **Time filter**. The **Transfer token** names the secret that holds the token; you never type the token itself.

## 3. Name the Scopes Your Data Is Filed Under

A scope files entities under a path, such as `/bratislava/ruzinov`, so a reader can ask for one district only. **Scope definitions** lists the paths your project uses. Click **New scope definition**, give a **Name** and the **Scope path**, and propose it. A scope level holds letters, digits and underscores; a hyphen is refused.

## 4. Describe Where Changes Are Promoted

An environment is another installation of the platform that this project's changes are promoted to, for example a test installation before production. **Environments** lists them. Click **New environment** and fill in the **Organization domain here**, the **Hosts** of each component, the **Image digests** it runs, the **Secret backend** and its mount, and the **Feature flags** that are on there. Then propose it.

## Related

- [Endpoints, Data Sharing & Integration](05-endpoints-and-sharing.md) — the Endpoints a data offer names.
- [Managing Your Organization and Projects](12-managing-organization-and-projects.md) — the same kinds for the whole organization, under Administration.
- [Users, Roles & Approvals](07-users-roles-approvals.md) — who approves the changes these forms propose.
