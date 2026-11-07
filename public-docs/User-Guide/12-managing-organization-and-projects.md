---
sidebar_position: 13
title: Managing Your Organization and Projects
description: Where the organization's settings, members, roles and projects live, and where one project's settings live.
routes:
  - /settings
  - /organization
  - /organization/$tab
  - /organization/$tab/$
  - /projects/$project/settings
  - /projects/$project/settings/$tab
  - /projects/$project/settings/$tab/$
  - /endpoints
  - /projects/$project/access
---

# Managing Your Organization and Projects

You manage access in two places. **Administration**, at `/organization`, holds what is the same in every project. **Project settings** holds what belongs to one project. Every change you make on either page is a proposed change that an approver merges, as everywhere else in the Portal ([07](07-users-roles-approvals.md)).

## 1. Administration

Administration is for organization administrators; anyone else reads "This page is for organization administrators" and asks one of them. Its side navigation has four groups:

| Group | Sections | What you do there |
|---|---|---|
| **People and access** | **People**, **Members**, **Roles**, **Groups**, **Service accounts**, **Agents** | See who belongs to the organization and with which role; add a person or a group, or remove them; read the roles in words and propose a new one; keep the groups you bind at once; the organization's service accounts and their keys; the agents acting for people. |
| **Projects and apps** | **Projects**, **All applications**, **All endpoints**, **Organization MCP servers**, **Organization models**, **Blueprints**, **Agent profiles** | Open, import, export or delete a project, and turn MQTT on for it; see every application and Endpoint of the organization in one list; keep the MCP servers, models, blueprints and agent profiles every project shares. |
| **Platform** | **Brokers**, **Environments**, **Data space**, **Health** | The brokers projects write to, the installations this one promotes changes to, the organization's data space membership ([Data Spaces, Scopes and Environments](19-data-spaces-scopes-environments.md)), and the health checks of every project at once. |
| **Settings** | **Settings**, **Setup** | The organization's domain, languages and contacts, who may open a project, the project quota; the first-run setup. |

An old link to `/endpoints` opens **All endpoints**. Deleting a project asks you to type its name, and the dialog lists everything the deletion removes.

## 2. Project settings

Open a project and choose **Settings**, the last item of its menu. **Find a setting** searches every tab, for example for `quota` or `delete`.

| Tab | What you do there |
|---|---|
| **General** | The project's title, description, who reads the projects and the catalogue publisher. |
| **Members** | Who is bound in this project or in one of its spaces. Give a person or a group an organization role or one of this project's roles. |
| **Roles** | This project's own roles, for example one that proposes pipelines but not endpoints. You can only write a role whose rights you hold yourself. |
| **Service accounts** | The project's service accounts and keys. |
| **Your access** | What you may read, propose, approve and delete in this project, and which binding gives it to you. |
| **Access explorer** | For organization administrators: who may do what in this project. |
| **Delete or hand over** | **Hand the project over** to another administrator, or **Delete project**: a red-lane change an administrator approves, with the list of what goes with it. |

An old link to `/projects/{project}/access` opens **Settings → Members**.

## 3. Which role is which

| You want someone to… | Use | Where |
|---|---|---|
| hold the same rights in every project | an organization role, bound at organization scope | Administration → Members |
| hold an organization role in one project only | the organization role, bound at project scope | Project settings → Members |
| hold rights your project defines | a project role | Project settings → Roles, then Members |
| use one application in a certain way | an application role | the application's own page |

Keycloak only knows who a person is and how they sign in. You never edit a role or a membership there.

## Related

- [07-users-roles-approvals.md](07-users-roles-approvals.md) — granting a role, approvals and drift, step by step.
- [Architecture/09 §14](../Architecture/09-portal.md#14-organization-and-project-management) — the specification of both pages.
- [Architecture/12 §2a](../Architecture/12-identity-and-access.md#2a-roles-as-code) — roles, bindings and groups as manifests.
