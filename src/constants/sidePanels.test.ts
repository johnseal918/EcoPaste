import { strict as assert } from "node:assert";
import { test } from "node:test";
import { sidePanelQuery } from "./sidePanels";

test("pinned panel preserves its independent manual order", () => {
  assert.deepEqual(sidePanelQuery("pinned"), {
    pinned: true,
    sort: "updatedAtDesc",
  });
});

test("favorite panel orders by recent time and still includes pinned favorites", () => {
  assert.deepEqual(sidePanelQuery("favorite"), {
    favorite: true,
    recentFirst: true,
    sort: "updatedAtDesc",
  });
});

test("text, image, and files panels use the same strict recent-first ordering", () => {
  for (const kind of ["text", "image", "files"] as const) {
    assert.deepEqual(sidePanelQuery(kind), {
      kind,
      recentFirst: true,
      sort: "updatedAtDesc",
    });
  }
});
