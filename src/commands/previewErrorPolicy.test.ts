import { strict as assert } from "node:assert";
import { test } from "node:test";
import {
  PREVIEW_HANDLE_TOAST_INTERVAL_MS,
  shouldReportPreviewHandleError,
} from "./previewErrorPolicy";

const show = "show_clipboard_preview";

test("native preview handle failures display at most one message per interval", () => {
  const error = "the underlying handle is not available";
  const first = 10_000;
  assert.equal(
    shouldReportPreviewHandleError(
      show,
      show,
      error,
      Number.NEGATIVE_INFINITY,
      first,
    ),
    true,
  );
  assert.equal(
    shouldReportPreviewHandleError(show, show, error, first, first + 16),
    false,
  );
  assert.equal(
    shouldReportPreviewHandleError(
      show,
      show,
      error,
      first,
      first + PREVIEW_HANDLE_TOAST_INTERVAL_MS,
    ),
    true,
  );
});

test("unrelated command errors and preview data failures are still reported", () => {
  assert.equal(
    shouldReportPreviewHandleError(
      "copy_item",
      show,
      "the underlying handle is not available",
      1_000,
      1_010,
    ),
    true,
  );
  assert.equal(
    shouldReportPreviewHandleError(
      show,
      show,
      "preview source window not found",
      1_000,
      1_010,
    ),
    true,
  );
});

test("bounded native recovery failures also use the same toast throttle", () => {
  assert.equal(
    shouldReportPreviewHandleError(
      show,
      show,
      "preview native window handle unavailable after bounded recovery",
      5_000,
      5_030,
    ),
    false,
  );
});
