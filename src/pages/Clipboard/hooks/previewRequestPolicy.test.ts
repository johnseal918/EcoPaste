import { strict as assert } from "node:assert";
import { test } from "node:test";
import {
  canStartPreviewOpen,
  PREVIEW_OPEN_FAILURE_COOLDOWN_MS,
} from "./previewRequestPolicy";

test("same card repeated pointer moves coalesce while its open is pending", () => {
  assert.equal(canStartPreviewOpen("a", "a", 0, 1_000), false);
  assert.equal(canStartPreviewOpen("b", "a", 0, 1_000), true);
  assert.equal(canStartPreviewOpen("a", null, 0, 1_000), true);
});

test("after a handle error retry waits rather than opening every frame", () => {
  const failedAt = 4_000;
  const retryAfter = failedAt + PREVIEW_OPEN_FAILURE_COOLDOWN_MS;
  assert.equal(canStartPreviewOpen("a", null, retryAfter, failedAt), false);
  assert.equal(canStartPreviewOpen("a", null, retryAfter, retryAfter - 1), false);
  assert.equal(canStartPreviewOpen("a", null, retryAfter, retryAfter), true);
});
