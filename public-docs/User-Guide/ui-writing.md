---
sidebar_position: 14
title: Writing the Portal's Words
description: The rules every label, message and control of the Portal follows, the word used for each concept, and the gate that checks them.
---

# Writing the Portal's Words

This page is for whoever writes or reviews the Portal's interface: a label, a button, a help line, an error. A city employee reads every one of them without the documentation open. The rules below hold in every locale the Portal ships (en, sk, cs, de). The gate `ui/tests/gate_human_text.test.ts` checks what a machine can judge. The walks T-3848 to T-3852 check the rest by hand, and `ui/tests/human_ui_walk.json` records who walked each module, when, and with which screenshots.

## 1. The Rules

1. **Plain words.** Write what a city employee says. A URN, a kind name such as `MqttBroker` or `ServiceAccount`, an HTTP status code, a JSON key, a UUID, a stack trace or a regular expression is never the main text. A technical value can appear only as secondary detail the reader can copy.
2. **Buttons say what happens.** A verb and its object: "Publish endpoint", "Revoke the consent". Never "Submit", "OK" or "Yes". A destructive action names what is lost and asks for confirmation with the object's name: "Delete the endpoint bikes?".
3. **Fields explain themselves.** Each field has a label, help in one sentence, an example, and a validation message that says how to fix the value ("Use 3 to 40 lowercase letters"), never "invalid". Required fields are marked.
4. **Errors say what to do next.** What happened, why, and the next step, with a link or a button to the fix. A refusal names the missing role in words ("You need the Steward role in this project; ask a project administrator"), never the Policy's internals.
5. **Lists are never blank.** Every list has an empty state (what would be here and the first action), a loading state, a failure state, and a count.
6. **Every action answers.** Success names the result and links to it. Long work shows progress and can be left and found again in the Inbox.
7. **Disabled controls say why.** In a tooltip that a screen reader reads too (`title` or `aria-describedby`). Nothing fails silently.
8. **Everyone can use it.** Complete by keyboard and screen reader, AA contrast, usable at 375 px and at 1440 px.

## 2. One Word per Concept

The Portal's glossary page (`/glossary`) explains the product's words. Use the same word everywhere, in the case shown, and never the manifest kind.

| Concept (kind) | Say | Never say |
|---|---|---|
| `Endpoint` | Endpoint | data access point, API, route |
| `Policy` | Policy | rule, ACL, grant document |
| `ContextSpace` | Context Space | namespace, tenant, partition |
| `Change` | change (proposed, merged) | pull request, commit, diff |
| `EntityType` | entity type | class, kind |
| `DataModel` | data model | LinkML schema, DataModel |
| `ServiceAccount` | service account | ServiceAccount, client, robot |
| `RoleBinding` | role (given to someone) | RoleBinding, binding |
| `McpServer` | MCP server | McpServer |
| `MqttBroker` | message broker | MqttBroker, MQTT client |
| `Pipeline` | pipeline | Bento config, stream |

Decided (worker-6, owner may override): the glossary's existing words stay (Endpoint, Policy, Context Space), because the Portal, the API and the documentation already name them so and the glossary explains each; a new word such as "data access point" would give one concept two names. Compound kind names become plain lowercase words.

## 3. What the Gate Checks

`gate_human_text.test.ts` runs in the fast `ci` lane beside the control gate. It fails when:

- a key is missing from any of en, sk, cs, de;
- an English string is a generic label (Submit, OK, Yes, Error, Invalid, Failed);
- a string shows a URN, a UUID, a kind name written as one CamelCase word, an HTTP status code, or JSON;
- an error string (its key names an error, a failure or a problem) has no next step: no second sentence and no instruction such as "try", "check" or "ask";
- a module disables more controls without a `title` or `aria-describedby` than it did.

What breaks a rule today is listed in `ui/tests/human_text.allow.json` with a reason. The list is the walks' work list and only shrinks: a new break is red, and an entry that no longer breaks anything is red until it is taken out. An entry that stays on purpose, such as a product name like LinkML or a code sample a developer copies, carries that reason instead of the baseline one.

## 4. The Walk

Each walk task takes one area, opens every control of it as listed in `ui/tests/controls.json`, and tries it: click, submit empty, submit wrong, submit right, cancel, without permission (`demo.viewer`), in conflict (two tabs), offline, and in an empty project. It fixes what breaks the rules, then records the module in `ui/tests/human_ui_walk.json` with who walked it, the date and the screenshots. The last walk sets `enforce` to `true`; from then on a module with controls and no walk fails the gate.

## Related

- [07-users-roles-approvals.md](07-users-roles-approvals.md): the roles a refusal names.
- [12-managing-organization-and-projects.md](12-managing-organization-and-projects.md): the pages most of the controls live on.
- [../STYLE.md](../STYLE.md): how the documentation itself is written.
