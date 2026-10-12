import { strict as assert } from "node:assert";
import { test } from "node:test";
import {
  normalizePreviewBackgroundOpacity,
  previewPanelBackgroundColor,
} from "./background";

test("100% preview is a solid theme color with no translucent overlay", () => {
  assert.equal(previewPanelBackgroundColor("#ffffff", 100), "#ffffff");
  assert.equal(previewPanelBackgroundColor("#141414", 100), "#141414");
});

test("50%-95% alpha-composites only the card background", () => {
  assert.equal(
    previewPanelBackgroundColor("#ffffff", 50),
    "color-mix(in srgb, #ffffff 50%, transparent)",
  );
  assert.equal(
    previewPanelBackgroundColor("#141414", 95),
    "color-mix(in srgb, #141414 95%, transparent)",
  );
});

test("out-of-range or legacy values normalize to the selected 5% steps", () => {
  for (const [input, expected] of [
    [NaN, 100],
    [0, 50],
    [51, 50],
    [73, 75],
    [99, 100],
    [120, 100],
  ]) {
    assert.equal(normalizePreviewBackgroundOpacity(input), expected);
  }
});
