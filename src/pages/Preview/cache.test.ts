import { strict as assert } from "node:assert";
import { test } from "node:test";
import type { ClipboardPreviewPayload } from "@/commands";
import {
  beginPreviewCacheSession,
  invalidateCachedPayload,
  readCachedPayload,
  writeCachedPayload,
} from "./cache";

function preview(id: string, text: string, updatedAt: string) {
  return {
    id,
    isSensitive: false,
    text,
    updatedAt,
  } as ClipboardPreviewPayload;
}

test("editing one item evicts all its cached versions but preserves other cards", () => {
  const cache = new Map<string, ClipboardPreviewPayload>();
  writeCachedPayload(cache, preview("edited", "before", "2026-10-12T01:00:00Z"), false);
  writeCachedPayload(cache, preview("other", "independent", "2026-10-12T01:00:00Z"), false);
  assert.equal(readCachedPayload(cache, "edited", false)?.text, "before");
  invalidateCachedPayload(cache, "edited");
  assert.equal(readCachedPayload(cache, "edited", false), null);
  assert.equal(readCachedPayload(cache, "other", false)?.text, "independent");
});

test("closing and reopening the preview starts with a fresh cache", () => {
  const cache = new Map<string, ClipboardPreviewPayload>();
  const original = preview("same-id", "old content", "old");
  const updated = preview("same-id", "saved content", "new");
  const first = beginPreviewCacheSession(cache, null, 17);
  writeCachedPayload(cache, original, false);

  // Repositioning the same preview must keep its cached content.
  assert.equal(beginPreviewCacheSession(cache, first, 17), 17);
  assert.equal(readCachedPayload(cache, "same-id", false)?.text, "old content");

  // A new session must not display old text before loading the DB.
  assert.equal(beginPreviewCacheSession(cache, 17, 18), 18);
  assert.equal(readCachedPayload(cache, "same-id", false), null);
  writeCachedPayload(cache, updated, false);
  assert.equal(readCachedPayload(cache, "same-id", false)?.text, "saved content");
});

test("replacing the cached revision never falls back to older content", () => {
  const cache = new Map<string, ClipboardPreviewPayload>();
  writeCachedPayload(cache, preview("a", "old", "v1"), false);
  writeCachedPayload(cache, preview("a", "saved", "v2"), false);
  assert.equal(cache.size, 1);
  assert.equal(readCachedPayload(cache, "a", false)?.text, "saved");
});

test("an update clears sensitive and unredacted versions together", () => {
  const cache = new Map<string, ClipboardPreviewPayload>();
  writeCachedPayload(cache, preview("a", "visible", "v1"), false);
  writeCachedPayload(cache, preview("a", "masked", "v1"), true);
  invalidateCachedPayload(cache, "a");
  assert.equal(readCachedPayload(cache, "a", false), null);
  assert.equal(readCachedPayload(cache, "a", true), null);
});
