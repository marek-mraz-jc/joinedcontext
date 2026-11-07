/**
 * T-2763: the assistant is told the page the person is on, and only a route the Portal reads as a
 * page of this project travels, so a question asked from anywhere else starts without one instead
 * of being refused.
 */
import { describe, expect, it } from "vitest";
import { pageContext } from "../src/assistant/state";

const at = (pathname: string, search = "") => pageContext("helsinki", { pathname, search });

describe("the page sent with a question", () => {
  it("is the route of a page of this project, with its tab", () => {
    expect(at("/projects/helsinki/spaces/helsinki", "?tab=inside&draft=x")).toEqual({
      route: "/projects/helsinki/spaces/helsinki?tab=inside",
    });
    expect(at("/projects/helsinki/endpoints/")).toEqual({ route: "/projects/helsinki/endpoints" });
    expect(at("/projects/helsinki/endpoints/bikes/edit")).toEqual({ route: "/projects/helsinki/endpoints/bikes/edit" });
  });

  it("is nothing off a page of this project, or on a route the Portal would refuse", () => {
    for (const pathname of [
      "/",
      "/organization/settings",
      "/projects/helsinki",
      "/projects/espoo/spaces",
      "/projects/helsinki/spaces/Helsinki",
      "/projects/helsinki/settings/members/add/more",
      `/projects/helsinki/${"a".repeat(64)}`,
    ]) {
      expect(at(pathname), pathname).toBeUndefined();
    }
    expect(at("/projects/helsinki/spaces", "?tab=Not%20a%20label")).toEqual({ route: "/projects/helsinki/spaces" });
  });
});
