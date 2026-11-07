---
sidebar_position: 23
title: Connect Claude Code, claude.ai, ChatGPT and VS Code
description: Create a scoped AI connector on My AI connectors, add it to Claude Code, claude.ai, ChatGPT or VS Code, what each preset allows, how a change waits for approval, what reaches the AI vendor, and what each refusal means.
routes:
  - /ai-connectors
---

# Connect Claude Code, claude.ai, ChatGPT and VS Code

An AI connector is an address of your own, `https://portal.{domain}/c/{id}`, that an AI client calls to work in the platform with the Portal's own tools: list and read objects, query data, prepare changes and run pipelines. You make it on **My AI connectors** at `/ai-connectors`, in your own menu, and choose what it reaches. It never reaches more than you may do yourself: the Portal reads your rights again on every call. Each client signs you in through the platform's login page; the page shows no token and nobody hands the client a password.

## 1. Create a Connector

1. Open **My AI connectors** and click **New AI connector**. Give it a **Name**, for example `doprava-in-claude`: lowercase letters, digits and dashes. Your AI client lists the connector by this name.
2. Pick one or more **Presets** (section 2). A greyed preset needs your organization's approval.
3. Under **What it reaches**, tick a whole project or single spaces, Endpoints or pipelines, each for read or write. The list holds only what you may read. A write that your organization does not allow yet shows as "write (needs approval)".
4. Choose when it **Expires**, at most 30 days from today unless your organization sets another limit.
5. Optionally tick the **Allowed clients**. Left empty, any AI client you sign in with can use it.
6. Click **Create**. The Portal checks every ticked item again; an item you may not read is refused beside the field.

The connector's card lists its state, what it reaches, its presets, clients, last call and expiry. **Set up** shows the address and what to paste into each client. A request above what your organization allows can wait as `pending` until an administrator approves it; it works only after that.

The address carries no right by itself. A call through it needs a token issued to you for this one connector, so a leaked address answers another person as if it did not exist. Still, keep it out of chats and screenshots.

## 2. What Each Preset Allows

| Preset | What the AI client may do on the ticked items |
|---|---|
| **Data read** | Read Endpoints, spaces and models, and query the data of the Endpoints, for example to answer questions about data. |
| **Context (models, spaces, Endpoints)** | As Data read, plus prepare changes to data models, context spaces, Endpoints and projections. |
| **Pipelines** | As Context for pipelines and data sources, plus sample a source, try a mapping, run a pipeline and read its logs. |
| **Access** | Read roles, bindings, groups and service accounts, and prepare changes to them. |
| **Publishing** | Read the catalogue and CKAN status, and prepare changes to what an Endpoint publishes. |
| **Apps** | Read Apps, dashboards and layers and their runs, and prepare changes to them. |
| **Operations** | Read activity, changes, runs, health and sync status, and run a pipeline. |
| **Everything (admin)** | Every preset above, within what your organization allows. |

A preset never adds a right you lack. A connector with presets **Pipelines** and one ticked pipeline reaches that pipeline only, and only as far as your own role in its project reaches.

## 3. Add It to Your AI Client

Open **Set up** on the connector's card and copy from there; the address below is a placeholder for yours. The first call opens the platform's login page in your browser. Sign in with your Portal account; the client keeps the sign-in for this connector only.

### 3.1 Claude Code

Copy the line under **Claude Code** and run it in a terminal:

```bash
claude mcp add --transport http doprava-in-claude https://portal.example.org/c/{id}
```

Then run `/mcp` inside Claude Code, pick `doprava-in-claude` and choose to authenticate. The browser opens the login page and returns to Claude Code on a port of your computer.

### 3.2 claude.ai and Claude Desktop

Claude Desktop uses the connectors of your claude.ai account, so one setup serves both.

1. On claude.ai open Settings, then Connectors, and choose Add custom connector.
2. Paste the address, choose Add, then Connect.
3. Sign in on the platform's login page; the browser returns to claude.ai.

Leave the OAuth Client ID and Client Secret fields empty: the platform recognises claude.ai by its published client id.

### 3.3 ChatGPT

1. In ChatGPT open Settings, then Apps & Connectors, then Advanced settings, and turn on Developer mode (your plan must offer it).
2. Choose Create, paste the address and choose OAuth as the authentication.
3. If ChatGPT asks for an OAuth Client ID, enter `mcp-mobile` and leave the secret empty.
4. Sign in on the platform's login page when it opens.

### 3.4 VS Code

Copy the entry under **VS Code (.vscode/mcp.json)** into the file `.vscode/mcp.json` of your workspace:

```json
{
  "servers": {
    "doprava-in-claude": {
      "type": "http",
      "url": "https://portal.example.org/c/{id}"
    }
  }
}
```

Start the server from the file or from the MCP view; VS Code opens the login page and keeps the sign-in.

### 3.5 Binding a Connector to One Client

**Allowed clients** names the clients that may use a connector. For claude.ai and ChatGPT the Portal can enforce it: no program on your computer can receive their sign-in. For Claude Code and VS Code it says whom the connector is for, and another program on your computer could name the same client; the form labels such a choice "says whom it is for".

## 4. Changes Wait for a Person's Approval

An AI client never changes the platform by itself. A change goes in two steps:

1. Plan: The client sends the change as a plan. The Portal runs every check a change in the Portal would run and saves nothing. The answer is the diff and each finding, for example a field the data model refuses, so the client can correct it. claude.ai, ChatGPT and VS Code show the plan as a view with a link to the project's **Approvals**.
2. Apply: When a ticked item is set to write, the client may apply its unchanged plan within 30 minutes. That opens a change request; it does not merge it. A plan whose base moved meanwhile is refused, and the client plans again.

A person approves or rejects the change request on **Approvals** in the Portal, as for any other draft. No connector can approve, revoke or widen a connector. With writes off, the client can still plan, and you apply the plan yourself in the Portal.

The AI client never sees a password or an API key. A tool call that carries a value that reads like one is refused with `secret-in-argument`. When a plan needs a credential, it names the Portal's secret form, where you type the value yourself (see [Entering a Secret a Plan Needs](08-working-with-ai-agents.md#8-entering-a-secret-a-plan-needs)).

## 5. What Leaves for the AI Vendor

Every answer the platform gives a client goes to that client's vendor: Anthropic for Claude Code, claude.ai and Claude Desktop, OpenAI for ChatGPT, and the model provider you chose in VS Code. Its model reads the answer beside your prompt, and the vendor keeps it as your plan's settings say.

The connector leaves out each attribute a data model marks as personal, such as a citizen's e-mail or phone number, and names what it left out without the values. Such attributes reach the client only where your organization allows it and the connector's scope names them. Text read from the data is marked as untrusted, so a client does not follow instructions hidden in it. Your data protection officer's assessment is [Data Protection Impact Assessment for AI Connectors](../Operations/04-ai-connectors-dpia.md).

## 6. Watch, Change and Revoke

- **Activity** lists every call through the connector in the last 30 days: when, the tool, its target, the client and the verdict. It never shows a token or the values a tool was given.
- **Edit** changes the presets, what it reaches, the expiry or the allowed clients; its name stays. The Portal checks the result as it checks a new connector.
- **Revoke**, then **Revoke now**, refuses the next call through the connector. A revoked connector cannot be turned on again; create a new one.

An administrator can see every person's connector on the organization overview and revoke one.

## 7. When a Call Is Refused

| What the client reports | Why | What to do |
|---|---|---|
| `401` with `WWW-Authenticate: Bearer resource_metadata=…` | The client has no sign-in for this connector, the sign-in expired, or the token was issued for another connector. | Authenticate again: `/mcp` in Claude Code, Connect in claude.ai, or remove and add the connector in ChatGPT. |
| `404`, the connector does not exist | The address is mistyped, or you signed in as another person than the connector's owner. | Copy the address again from **Set up** and sign in with your own account. |
| `403` "this connector is revoked" or "expired" | The connector was revoked or its expiry passed. | For an expired one, **Edit** and pick a later day under **Expires**. A revoked one stays off: create a new one. |
| `403` "this connector answers only …; the token was obtained by …" | The connector's **Allowed clients** do not include the client you used. | Use a listed client, or **Edit** the connector's **Allowed clients**. |
| `403` naming a tool, a kind or a target, out of scope | The call reaches a tool or an item the connector's presets and ticks do not cover, or your own role does not allow it. | **Edit** the connector's **Presets** or **What it reaches**. If your role lacks the right, ask a project administrator; see [Users, Roles and Approvals](07-users-roles-approvals.md). |
| `422` `secret-in-argument` | A tool argument holds a value that reads like a password or key. | Enter the credential in the Portal's secret form; never type it into the chat. |
| `429` with `Retry-After` | The connector had more than 4 calls running at once, or you had more than 8 across all your connectors. | Wait the seconds `Retry-After` names; ask the client for fewer calls at once. |
| A tool answer is too large (`413`) | One answer would pass 1 MiB. | Narrow the read with `limit`, `attrs` or one name. |

## 8. The Portal's Shared MCP Address

Besides your own connectors, the Portal has one MCP address for its whole tool set, `https://portal.{domain}/api/v1/mcp`, which acts with all of your rights. A client adds it with the OAuth Client ID `mcp-mobile` (claude.ai, Claude Desktop, ChatGPT) or `mcp-local` (Claude Code and other clients on your computer, with the callback `http://localhost:{port}/callback`), no secret, and the scope `openid mcp:portal` where a client asks for one. A connector is the better choice: it reaches only what you tick and you can revoke it alone.

## Related

- [Working with AI Agents](08-working-with-ai-agents.md): the assistant, agent runs and the Endpoint MCP addresses
- [Users, Roles and Approvals](07-users-roles-approvals.md)
- [Data Protection Impact Assessment for AI Connectors](../Operations/04-ai-connectors-dpia.md)
- [Connectors API](../API/14-connectors.md)
- [Agents and MCP](../Architecture/07-agents-and-mcp.md)
