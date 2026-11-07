---
sidebar_position: 20
title: Health, Activity and Your Inbox
description: Where to see whether a project works, what happened in it, and what waits for your decision.
routes:
  - /projects/$project/health
  - /projects/$project/activity
  - /projects/$project/inbox
---

# Health, Activity and Your Inbox

Three pages answer the questions you ask every morning: does the project work (**Health**), what happened in it (**Activity**), and what waits for you (**Inbox**). They are written for data stewards and approvers; anyone who may read the project can open the first two.

## 1. Is Everything Working: Health

Open a project and choose **Health**. The page lists every check of the project's pipelines, Context Spaces, Endpoints, models and data sources, errors first. The Portal runs the checks every few minutes by itself.

1. Narrow the list with **Show**, for example to **Pipeline**, or keep **All kinds**.
2. Read each row: **Status**, the **Resource** it checked, and **What it found**, such as "not applied yet" or "waiting for approval".
3. Follow the advice after "Fix:". It names the step to take, for example to check a data source's address when the source did not answer.
4. **Last 7 days** shows whether the problem is new or keeps coming back; **Last ok** says when the check last passed, or that it never did.

A check covers one thing: whether the resource is applied, whether its runs succeed, whether its data is fresh, whether entities arrive, how much storage it uses, whether the data keeps the model's rules, and whether the source is reachable.

When the page says **Nothing checked yet**, the Portal has not run its first checks: add a pipeline, a Context Space or an Endpoint, or come back in a few minutes. **All checks are ok** means there is nothing to fix.

## 2. What Happened: Activity

**Activity** shows what happens in the project, newest first: changes applied, pipeline runs, requests served and refused, and catalogue publishing.

1. Read the summary at the top, **What happened in the last hour**: **Messages ingested**, **Changes applied**, **Requests served**, **Forwards** and **Refusals**.
2. Narrow the feed with **Kind**, **Source**, **Severity** and **Context Space**. **Any** removes a filter.
3. **Live tail** keeps adding new events as they happen; it shows **Following** while it runs and **Paused** when you stop it, so the row you are reading does not move.
4. **Show older** pages back in time. **Technical details** on a row shows the full event, which helps when you report a problem.

An event that repeats is shown once with how many times it happened since when. A refused request, for example **Access refused**, tells you which reader your Policies kept out; [Endpoints, Data Sharing & Integration](05-endpoints-and-sharing.md) explains how to change who may read.

## 3. What Waits for You: Inbox

**Inbox** collects, across every project you may read, the changes you may decide and the comments that mention you.

1. Under **Waiting for your decision**, open **Show what it changes** to read the change without leaving the page, or **Open the change** for its full page.
2. Click **Approve**. A red-lane change asks you to type the resource's name first, because it makes something public or removes something.
3. Or click **Reject**, write under **Why are you rejecting this?** what the author must fix, and click **Reject the change**. The author reads your reason on the change.
4. Under **Mentions of you**, open the comment someone wrote about you in a space.

**Approve** and **Reject** work only where your role lets you decide changes; elsewhere you can read the change and leave the decision to an approver. When the page says a project's changes did not load, it tries again by itself in 30 seconds.

## Related

- [Users, Roles & Approvals](07-users-roles-approvals.md) — how a change is reviewed and who may decide it.
- [Pipelines](04-pipelines.md) — pausing a pipeline and reading its stream health.
- [Retention and Expiry](14-retention-and-expiry.md) — when a storage warning means old data should go.
