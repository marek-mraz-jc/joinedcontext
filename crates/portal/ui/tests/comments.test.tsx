/**
 * Comments on an entity with @mentions, and the person's notifications (T-3106, API/01 §35): the
 * panel lists, posts and removes only one's own; a mention the API did not notify is said; the
 * header's menu counts the unread and marks one read when it is opened.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import i18n from "../src/i18n";
import { CommentsPanel, NotificationsMenu } from "../src/pages/spaces/Comments";

const navigate = vi.fn();
vi.mock("@tanstack/react-router", async (original) => ({
  ...(await original<typeof import("@tanstack/react-router")>()),
  useNavigate: () => navigate,
}));

const URN = "urn:ngsi-ld:AirQualityObserved:hel.fi:air:1";

const json = (body: unknown, status = 200) =>
  Promise.resolve(
    status === 204
      ? new Response(null, { status })
      : new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }),
  );

function wrap(children: ReactNode) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </QueryClientProvider>,
  );
}

const comment = (id: number, mine: boolean, text: string) => ({
  id,
  urn: URN,
  author: mine ? "sami" : "anna",
  authorName: mine ? "Sami" : "Anna",
  text,
  mentions: [],
  createdAt: "2026-10-06T19:20:00Z",
  mine,
});

/** The requests the page sent to paths ending in `tail`, in order. */
function sentTo(fetchMock: ReturnType<typeof vi.fn>, tail: string): Request[] {
  return fetchMock.mock.calls.map((call) => call[0] as Request).filter((request) => new URL(request.url).pathname.endsWith(tail));
}

describe("comments on an entity", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("lists the comments, posts one with its mentions and says who was not notified", async () => {
    let listed = [comment(1, false, "pm10 jumped")];
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      if (request.method === "POST") {
        listed = [...listed, comment(2, true, "@anna@hel.fi @nobody@x.org seen")];
        return json({ ...listed[1], unknownMentions: ["nobody@x.org"] }, 201);
      }
      return json(listed);
    });
    vi.stubGlobal("fetch", fetchMock);
    wrap(<CommentsPanel project="helsinki" space="air" urn={URN} />);

    const list = await screen.findByRole("list", { name: "Comments" });
    expect(within(list).getByText("pm10 jumped")).toBeInTheDocument();
    // Another person's comment cannot be removed from here.
    expect(within(list).queryByRole("button", { name: /Remove/ })).toBeNull();
    expect(new URL(sentTo(fetchMock, "/spaces/air/comments")[0].url).searchParams.get("urn")).toBe(URN);

    await userEvent.type(screen.getByLabelText("Add a comment"), "@anna@hel.fi @nobody@x.org seen");
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(await screen.findByText("Not notified, because they may not read this space: @nobody@x.org")).toBeInTheDocument();
    const posted = sentTo(fetchMock, "/spaces/air/comments").find((request) => request.method === "POST");
    expect(await posted?.json()).toEqual({ urn: URN, text: "@anna@hel.fi @nobody@x.org seen" });
    await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(2));
    expect(screen.getByLabelText("Add a comment")).toHaveValue("");
  });

  it("removes one's own comment and says a refusal in the API's words", async () => {
    let refuse = false;
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      if (request.method === "DELETE") {
        return refuse ? json({ title: "Not Found", status: 404, detail: "no comment 3 of yours here" }, 404) : json(null, 204);
      }
      return json([comment(3, true, "mine")]);
    });
    vi.stubGlobal("fetch", fetchMock);
    wrap(<CommentsPanel project="helsinki" space="air" urn={URN} />);
    const remove = await screen.findByRole("button", { name: /Remove your comment of/ });
    await userEvent.click(remove);
    await waitFor(() => expect(sentTo(fetchMock, "/spaces/air/comments/3").map((r) => r.method)).toEqual(["DELETE"]));
    refuse = true;
    await userEvent.click(remove);
    expect(await screen.findByRole("alert")).toHaveTextContent("no comment 3 of yours here");
  });

  it("does not post an empty comment", async () => {
    vi.stubGlobal("fetch", vi.fn(() => json([])));
    wrap(<CommentsPanel project="helsinki" space="air" urn={URN} />);
    expect(await screen.findByText("No comments yet.")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Add a comment"), "   ");
    expect(screen.getByRole("button", { name: "Comment" })).toHaveAttribute("aria-disabled", "true");
  });
});

describe("notifications", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    navigate.mockReset();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("lists an alert notice beside the mentions, counts it, and mutes its subscription for a day (T-3261)", async () => {
    const notice = { id: 9, subscription: 4, project: "helsinki", pipeline: "bikes", event: "zero", change: "opened", detail: "records in, nothing out", createdAt: "2026-10-07T08:00:00Z", read: false };
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      const path = new URL(request.url).pathname;
      if (request.method === "POST") return json(path.endsWith("/mute") ? { id: 4 } : null, path.endsWith("/mute") ? 200 : 204);
      return path.endsWith("/alerts/notices") ? json({ items: [notice], unread: 1 }) : json({ items: [], unread: 0 });
    });
    vi.stubGlobal("fetch", fetchMock);
    wrap(<NotificationsMenu project="helsinki" />);
    await userEvent.click(await screen.findByRole("button", { name: "Notifications, 1 unread" }));
    expect(await screen.findByRole("menuitem", { name: /bikes: it writes nothing/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("menuitem", { name: "Mute the alerts of bikes for a day" }));
    await waitFor(() => expect(sentTo(fetchMock, "/alerts/4/mute").map((r) => r.method)).toEqual(["POST"]));
  });

  it("counts the unread on the button, lists them, and marks one read when it is opened", async () => {
    const item = {
      id: 5,
      kind: "mention",
      project: "helsinki",
      space: "air",
      urn: URN,
      commentId: 1,
      author: "anna",
      authorName: "Anna",
      excerpt: "@sami look at pm10",
      createdAt: "2026-10-06T19:20:00Z",
      read: false,
    };
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      if (request.method === "POST") return json(null, 204);
      // The alert notices are the inbox's second list (T-3261); this test is about mentions.
      return new URL(request.url).pathname.endsWith("/alerts/notices")
        ? json({ items: [], unread: 0 })
        : json({ items: [item], unread: 1 });
    });
    vi.stubGlobal("fetch", fetchMock);
    wrap(<NotificationsMenu project="helsinki" />);
    const button = await screen.findByRole("button", { name: "Notifications, 1 unread" });
    await userEvent.click(button);
    const entry = await screen.findByRole("menuitem", { name: /Anna mentioned you in air/ });
    await userEvent.click(entry);
    await waitFor(() => expect(sentTo(fetchMock, "/notifications/5/read").map((r) => r.method)).toEqual(["POST"]));
    expect(navigate).toHaveBeenCalledWith({ href: "/projects/helsinki/spaces/air" });
  });

  it("words each connector event by its kind and opens its own Portal page (T-5979)", async () => {
    const event = (id: number, kind: string, link: string) => ({ id, kind, project: "helsinki", space: "", urn: "cr-12", commentId: null, author: "marek", authorName: "Marek", excerpt: "doprava-in-claude", link, createdAt: "2026-10-28T20:00:00Z", read: false });
    const items = [
      event(1, "change.opened", "/projects/helsinki/approvals/cr-12"),
      event(2, "connector.pending", "/ai-connectors"),
      // A link off the Portal is never followed: the item opens its space instead.
      event(3, "connector.expiring", "//evil.example/x"),
    ];
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      if (request.method === "POST") return json(null, 204);
      return new URL(request.url).pathname.endsWith("/alerts/notices") ? json({ items: [], unread: 0 }) : json({ items, unread: 3 });
    });
    vi.stubGlobal("fetch", fetchMock);
    wrap(<NotificationsMenu project="helsinki" />);
    await userEvent.click(await screen.findByRole("button", { name: "Notifications, 3 unread" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /Marek opened change cr-12 in helsinki through connector doprava-in-claude/ }));
    expect(navigate).toHaveBeenCalledWith({ href: "/projects/helsinki/approvals/cr-12" });
    await userEvent.click(await screen.findByRole("button", { name: /Notifications/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /Marek asks for connector doprava-in-claude above the organization ceiling/ }));
    expect(navigate).toHaveBeenCalledWith({ href: "/ai-connectors" });
    await userEvent.click(await screen.findByRole("button", { name: /Notifications/ }));
    await userEvent.click(await screen.findByRole("menuitem", { name: /Your connector doprava-in-claude ends within 7 days/ }));
    expect(navigate).toHaveBeenLastCalledWith({ href: "/projects/helsinki/spaces/" });
  });

  it("says when there is nothing new", async () => {
    vi.stubGlobal("fetch", vi.fn(() => json({ items: [], unread: 0 })));
    wrap(<NotificationsMenu project="helsinki" />);
    await userEvent.click(await screen.findByRole("button", { name: "Notifications" }));
    expect(await screen.findByText("Nothing new.")).toBeInTheDocument();
  });
});
