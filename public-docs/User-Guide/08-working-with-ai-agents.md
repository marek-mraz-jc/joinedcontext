---
sidebar_position: 9
title: Collaborating with Autonomous AI Agents
description: Using the docked assistant, running unattended agent tasks, inspecting agent access, and connecting outside tools via MCP.
routes:
  - /projects/$project/assistant
  - /projects/$project/mcp
  - /projects/$project/knowledge
  - /projects/$project/knowledge/conversations
  - /projects/$project/knowledge/$source
  - /projects/$project/knowledgesources
  - /projects/$project/assistantdeployments
  - /projects/$project/agentprofiles
  - /connectors
  - /ai-connectors
  - /projects/$project/mcp-elicitations/$id
  - /mcp-confirmations/$id
  - /projects/$project/secrets/new
---

# Collaborating with Autonomous AI Agents

The Portal treats AI agents as team members governed by the principle of least privilege. You can converse with the assistant from any screen, launch background agent work runs, inspect runtime permissions, and connect external AI clients over the Model Context Protocol (MCP). Every change prepared by an agent lands as a draft that requires human review and approver sign-off.

```mermaid
flowchart TD
    USER["User Request"] --> ASSISTANT["Assistant Run"]
    ASSISTANT --> INTERSECT{"Permissions Check<br/>Profile Grant ∩ User Rights"}
    INTERSECT -->|Permitted| DRAFT["Draft Manifest in Portal"]
    INTERSECT -->|Denied| REFUSE["Refusal Message"]
    DRAFT --> REVIEW["Human Reviews & Proposes"]
    REVIEW --> APPROVAL["Approver Merges Change"]
```

## 1. Using the Docked Assistant

The assistant is accessible from every page through a persistent button fixed at the bottom right corner of the screen.

### Asking Questions and Driving Portal Navigation

#### By hand

1. Click **Open the assistant** in the bottom right corner of any page.
2. In the text area labelled **Tell the assistant what to build or change…**, type your query, such as asking about platform activity.
3. Press Enter or click **Send**.
4. Use the dock header controls to manage the conversation:
   - Click **Stop the assistant** to interrupt an active response.
   - Click **Full screen** to expand the conversation view across the workspace.
   - Click **Hide the assistant** to fold the panel into the bubble while preserving conversation context.
   - Click **Close the assistant** to release the active dialogue.
You should see the assistant stream tool invocations, answer your questions, and navigate to target forms with pre-filled drafts.

The live journeys `assistant-reads.spec.ts` and `assistant-creates.spec.ts` replay these steps.

#### By asking the assistant

Type into the composer: `Create a context space called air-quality in the helsinki project`. The assistant switches routes to `/projects/helsinki/spaces`, opens the **New Context Space** dialog, and populates the name and model fields. You run the validation check and click **Propose change** yourself.

## 2. Launching Unattended Agent Work

From the dedicated Assistant workbench, you can launch autonomous runs that generate applications, dashboards, or data analyses without step-by-step interview prompts.

### Starting a Background Work Run

#### By hand

1. In the sidebar navigation, click **Assistant** to open `/projects/helsinki/assistant`.
2. Scroll down to the **New work** card.
3. In **What to make**, choose `Application`, `Dashboard`, or `Analysis`.
4. In **Name**, enter `bikes-summary`.
5. In **Endpoint**, select a published data view, such as `helsinki-bikes`.
6. In **What should it do?**, describe the expected layout and metrics, such as `Show total available bikes and station capacity`.
7. Click **Start**.
You should see the run appear in the table with its current phase, displaying elapsed execution time and continuation options once completed.

The live journey `build-samples.spec.ts` replays these steps.

#### By asking the assistant

Type into the composer: `Build an application called bikes-summary from the helsinki-bikes endpoint showing total available bikes`. The assistant initiates the run and provides a direct link to open the execution view.

## 3. Reviewing and Editing Agent Access

An agent profile defines the outer boundary of operations an agent may reach. Runtime access is always calculated as the strict intersection of the profile grant and your personal permissions.

### Inspecting Profile Capabilities

#### By hand

1. Open `/projects/helsinki/assistant` and scroll to the **Agent access** section.
2. Select a profile card, such as `app-builder`.
You should see a table listing operations, indicating whether the profile grants each capability, whether your user account possesses it, and any applicable restriction reasons.
3. Click **Edit access** to open the access configuration editor.
4. Update the YAML specification under **Access block (YAML)** to add or remove permitted operations.
5. Click **Propose change** to submit the modification as a proposal in Approvals.

#### By asking the assistant

The assistant cannot widen its own permissions or alter its assigned profile directly. You must inspect and update profile grants by hand in the Portal.

## 4. Connecting External Agents via MCP

Published endpoints provide Streamable HTTP endpoints implementing the Model Context Protocol, enabling external agents like Claude Code or OpenHands to query context data.

### Configuring an External Agent Connection

#### By hand

1. Open `/projects/helsinki/endpoints` and select an endpoint with the `mcp` representation enabled, such as `helsinki-all`.
2. Locate the public URL and copy the MCP address: `https://api.joinedcontext.com/api/endpoint/mluyob4nz52lok3ssk7pgn5vwt/mcp`.
3. In your local MCP client configuration, add the server entry:

   ```json
   {
     "mcpServers": {
       "helsinki-context": {
         "url": "https://api.joinedcontext.com/api/endpoint/mluyob4nz52lok3ssk7pgn5vwt/mcp"
       }
     }
   }
   ```

4. Connect your client. A public endpoint answers the tool list and every read with no sign-in at all. An endpoint published to the organization or to selected projects refuses an anonymous call and tells the client where to sign in, so your client runs that sign-in once and comes back with a token that is good for this one endpoint.
You should see your external agent discover available data inspection tools and execute queries conforming to the endpoint policy.

#### By asking the assistant

Type into the composer: `What is the MCP address for the helsinki-all endpoint?`. The assistant inspects the endpoint manifest and returns the complete Streamable HTTP URL ready to paste into your external client configuration.

### Deciding a Call Your Client Asks About

When your AI client asks to run an MCP call that changes the project, the call does not run yet. The client shows you a link to the page **Decide an AI client's call** in the Portal, where you sign in with your own session.

1. Read the client's message, the operation it names and the time the question stays open until (ten minutes).
2. Open **Look at what this call changes** to see the resource the call would change.
3. Select **Accept the call** to let this one call run once, or **Decline the call** to stop it. You can change your answer until the client sends the call again.
4. Go back to your AI client. Its next try of the same call runs, or tells the client you declined.

Nothing the client says counts as an answer: only your choice on this page does. If the page says **This question is no longer open**, the question was answered, it expired, or another person's client asked it: ask your client to send its call again.

A write through an Endpoint's own MCP address, which the context broker answers, asks the same way on the page `/mcp-confirmations/<id>`: it shows the view and the tool the write goes through instead of an operation, and has no link to a Portal page. The broker runs the write only on the answer you record there; the page answers only the person whose client asked.

## 5. Knowledge Sources and Assistants

An assistant answers from the knowledge sources you give it: a website, a data catalogue, this project's Context Spaces, the User Guide or files you upload. **Knowledge sources**, at `/projects/{project}/knowledge`, lists them with what the assistant has read, and lists the project's **Assistants** below.

### Add a source and an assistant

1. Click **Add a source**. Choose its **Type**, for example **Website**, give the **Start addresses** such as `https://www.example.sk/`, and limit what is read with **Read only these paths**, **Skip these paths**, **Link depth** and **Most pages**. Under **Who may be answered from it** choose **Internal: signed-in staff** or **Public: every channel**.
2. Click **Propose change**. Once an approver approves it, the source is read within a few minutes. **Crawl now** reads it again on demand.
3. Click **Add an assistant**. Give it a **Name**, a **Channel** (**Public**, **Internal (Portal)**, **Data catalogue** or **Embedded on a site**), the **Sources** it answers from and its **Instructions**, and propose it. On a public channel, set **Allowed sites** and a **Budget**.
4. On the assistant's row, **Ask** tries it, **Embed** gives the snippet for your website, **Duplicate** starts a variant and **Usage** shows questions and tokens per day.

The same kinds are listed as tables under **Knowledge sources** (`/projects/{project}/knowledgesources`), **Assistant deployments** (`/projects/{project}/assistantdeployments`) and **Agent profiles** (`/projects/{project}/agentprofiles`), with a button to add one and the same forms; an agent profile is described in section 3.

### Decide what the assistant answers from

Click a source to open its page. **Pages** and **Documents** list everything that was read, with its state: **Read**, **Skipped**, **Could not be read** or **Not read yet**.

1. Find a page with **Find pages**, tick it, and click **Leave out of answers**. Tick **With every page below a selected page, and the documents they link** to leave out a whole part of the site. **Take back into answers** undoes it.
2. Under **Try the search**, type a **Question or words** and click **Try the search** to see which passages the assistant would answer from.
3. For a source of uploaded files, use **Choose files to upload** and **Upload files**: PDF, DOCX, XLSX, CSV, Markdown or text, at most 50 MB each.

A page with **No passage** is never used in an answer.

## 6. Reading Stored Conversations

**Conversations**, linked from the knowledge page, shows what visitors asked a public assistant and how each answer was made. Only a person who approves the project's assistant deployments may read it, and every read is recorded; conversations are kept for 90 days.

1. Narrow the list by **Words in a question or answer**, **Status**, **Rating**, **Assistant** and dates, or sort it, for example **Most expensive first**.
2. Open a conversation to read its **Steps**: what the assistant searched, read and answered.
3. **Same question** and **Compare** set two answers side by side. **Copy as test** turns a turn into a test case for the assistant.
4. **Erase the conversation** deletes it when a visitor asks for it. This cannot be undone.
5. The usage panel totals **Questions** and **Total cost** for the range, with **Download CSV**.

## 7. MCP Servers and Your Own Connectors

**MCP servers**, at `/projects/{project}/mcp`, bundles several Endpoints behind one address for AI clients.

1. Click **New MCP server**. Give it a **Name** and **Title**, add Member Endpoints (find them with **Find an Endpoint**), optionally **Knowledge sources**, and choose **Who may connect**: **Listed projects**, **Organization** or **Public**.
2. **Tools you would get** lists the reads and writes the server will offer. Click **Check and propose**. A public server needs an approver who may publish.
3. On the server's card, copy the **Address** or the **Entry for the AI client's configuration**. A public server also has **Open the public connect page**, the page described in [The Catalogue and Public Pages](17-catalogue-and-public-pages.md#5-connect-an-ai-assistant-to-public-data).

The old address of your own connectors, `/connectors`, now opens **My AI connectors**.

**My AI connectors**, at `/ai-connectors` in your own menu, holds addresses for Claude Code, claude.ai, ChatGPT or VS Code that reach the Portal's own tools, up to 20. Each one reaches only what you tick, and never more than you may do yourself: the Portal reads your rights again at every call. The page shows no token; each AI client signs you in itself.

1. Click **New AI connector** and give it a **Name** such as `doprava-in-claude`.
2. Pick the **Presets**, such as **Data read**. A greyed preset needs your organization's approval.
3. Under **What it reaches**, tick a whole project or single spaces, Endpoints or pipelines. The list holds only what you may read, and the Portal checks every tick again when you click **Create**.
4. Choose when it **Expires**, at most 30 days from today, and optionally the **Allowed clients**.
5. Click **Create**. Under **Set up**, copy the `claude mcp add` line for Claude Code, the entry for VS Code, or follow the steps for claude.ai and ChatGPT.

**Activity** lists every call through a connector in the last 30 days. **Edit** changes what it reaches or when it expires; its name stays. **Revoke**, then **Revoke now**, refuses the next call through it. [Connect Claude Code, claude.ai, ChatGPT and VS Code](21-connect-ai-clients.md) gives each client's steps, what each preset allows and what each refusal means.

## 8. Entering a Secret a Plan Needs

A source often needs an API key or a password. Never type one into a chat with the assistant or an AI connector: the Portal refuses any tool call that carries a credential-shaped value and answers `secret-in-argument`, naming the field and never the value. When a plan needs a credential, it lists it under `needsSecret` with a link to the secret form at `/projects/{project}/secrets/new`.

1. Open the link. **Secret name** and **Key** come filled from the plan, for example `counters-api` and `apiKey`.
2. Paste the credential into **Value**. Nobody can read it back on this page.
3. Click **Encrypt and propose**. The Portal encrypts the value into the project's repository and opens a change for an approver; the plan then names the secret only by `secretRef`.

## Related

- [07-users-roles-approvals.md](./07-users-roles-approvals.md): reviewing merge proposals and managing user access.
- [11-apps.md](./11-apps.md): supervising application builder agents and evaluating previews.
- [10-role-guides.md](./10-role-guides.md): role-specific workflows and platform navigation cheat sheets.
