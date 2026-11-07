// The Apps' coverage gate reads which controls the tests rendered and which they exercised (T-3373).
import { afterAll } from "vitest";
import { recordControls } from "@joinedcontext/sdk/testing";

recordControls(afterAll);
