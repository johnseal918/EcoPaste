import { useEventListener } from "ahooks";
import type { PointerEvent as ReactPointerEvent } from "react";
import { useEffect, useRef, useState } from "react";
import { useSnapshot } from "valtio";
import { showClipboardPreview } from "@/commands";
import { TAURI_EVENT } from "@/constants/events";
import { WINDOW_LABEL } from "@/constants/windows";
import { useKeyboardEvent } from "@/hooks/useKeyboardEvent";
import { useTauriListen } from "@/hooks/useTauriListen";
import { settingsState } from "@/stores/settings";
import type { ClipboardItem } from "@/types/clipboard";
import { log } from "@/utils/log";
import {
  clearHoverTimer,
  closeClipboardPreviewSilently,
  HOVER_DELAY_MS,
  HOVER_HIDE_BUFFER_MS,
  isSpaceKey,
  type PreviewSession,
  type PreviewTrigger,
  type UseClipboardPreviewControllerOptions,
  type WindowVisibilityPayload,
} from "./previewController";
import {
  canStartPreviewOpen,
  PREVIEW_OPEN_FAILURE_COOLDOWN_MS,
} from "./previewRequestPolicy";

const KEYBOARD_PREVIEW_MAX_FRAMES = 36;
const KEYBOARD_PREVIEW_STABLE_FRAMES = 2;
const KEYBOARD_PREVIEW_RECT_EPSILON = 0.5;

interface PreviewRectSnapshot {
  left: number;
  top: number;
  width: number;
  height: number;
}

interface KeyboardPreviewTarget {
  frames: number;
  item: ClipboardItem;
  lastRect: PreviewRectSnapshot | null;
  stableFrames: number;
}

/**
 * 管理剪贴板预览的 hover / keyboard 生命周期和窗口隐藏兜底。
 */
export function useClipboardPreviewController(
  options: UseClipboardPreviewControllerOptions,
) {
  const { getActiveItem, itemElementMapRef, onHoverSelect, sourceWindowLabel } =
    options;
  const [previewSession, setPreviewSession] = useState<PreviewSession | null>(
    null,
  );
  const hoverTimerRef = useRef<number | null>(null);
  const hoverHideTimerRef = useRef<number | null>(null);
  const clipboardWindowVisibleRef = useRef(true);
  const previewSessionRef = useRef<PreviewSession | null>(null);
  const previewOpenRequestIdRef = useRef(0);
  const previewOpenInFlightRef = useRef<{
    itemId: string;
    requestId: number;
  } | null>(null);
  const previewOpenRetryAfterRef = useRef(0);
  const previewMoveFrameRef = useRef<number | null>(null);
  const previewMoveTargetRef = useRef<{
    item: ClipboardItem;
    pointerY: number;
  } | null>(null);
  const keyboardPreviewFrameRef = useRef<number | null>(null);
  const keyboardPreviewTargetRef = useRef<KeyboardPreviewTarget | null>(null);
  const pendingHoverTargetRef = useRef<{
    item: ClipboardItem;
    pointerY: number;
  } | null>(null);
  // A WebView can synthesize pointerenter when a hidden window becomes
  // visible or list rows are re-mounted under a stationary cursor. Hover
  // previews must be triggered by real pointer movement in this session.
  const lastPointerLocationRef = useRef<{ x: number; y: number } | null>(null);
  const settingsSnapshot = useSnapshot(settingsState);
  const previewSettings = settingsSnapshot.clipboard.preview;

  useEffect(() => {
    if (!previewSettings.hoverEnabled && previewSession?.trigger === "hover") {
      clearHoverTimer(hoverTimerRef);
      clearHoverTimer(hoverHideTimerRef);
      previewSessionRef.current = null;
      setPreviewSession(null);
      closeClipboardPreviewSilently("hoverDisabled", sourceWindowLabel);
      return;
    }

    if (
      !previewSettings.spaceEnabled &&
      previewSession?.trigger === "keyboard"
    ) {
      clearHoverTimer(hoverTimerRef);
      clearHoverTimer(hoverHideTimerRef);
      previewSessionRef.current = null;
      setPreviewSession(null);
      closeClipboardPreviewSilently("spaceDisabled", sourceWindowLabel);
    }
  }, [
    previewSettings.hoverEnabled,
    previewSettings.spaceEnabled,
    previewSession?.trigger,
    sourceWindowLabel,
  ]);

  useEffect(() => {
    return () => {
      clearHoverTimer(hoverTimerRef);
      clearHoverTimer(hoverHideTimerRef);
      pendingHoverTargetRef.current = null;
      previewOpenInFlightRef.current = null;
      if (keyboardPreviewFrameRef.current !== null) {
        window.cancelAnimationFrame(keyboardPreviewFrameRef.current);
        keyboardPreviewFrameRef.current = null;
      }
      keyboardPreviewTargetRef.current = null;
      if (previewMoveFrameRef.current !== null) {
        window.cancelAnimationFrame(previewMoveFrameRef.current);
        previewMoveFrameRef.current = null;
        previewMoveTargetRef.current = null;
      }
    };
  }, []);

  const handleWindowVisibility = (event: {
    payload: WindowVisibilityPayload;
  }) => {
    const { label, visible } = event.payload;
    // Side hosts have their own WebViews. The main-window lifecycle and our
    // own host lifecycle can both invalidate a hover preview.
    if (label !== WINDOW_LABEL.CLIPBOARD && label !== sourceWindowLabel) return;

    clipboardWindowVisibleRef.current = visible;
    lastPointerLocationRef.current = null;
    // Reset on SHOW as well as HIDE. Dormant WebViews can miss the hide event;
    // never resume an old timer, hover target or pending native show request.
    closePreview(visible ? "windowShownReset" : "windowHidden");
  };

  useTauriListen(TAURI_EVENT.WINDOW_VISIBILITY, handleWindowVisibility);

  const handleWindowBlur = () => {
    lastPointerLocationRef.current = null;
    // A pending hover timer is not represented by previewSessionRef yet.
    // Cancel it even if the preview has not opened.
    if (hoverTimerRef.current !== null || previewSessionRef.current) {
      closePreview("windowBlur");
    }
  };

  useEventListener("blur", handleWindowBlur, { target: window });

  const handleWindowResize = () => {
    lastPointerLocationRef.current = null;
    closePreview("windowResize");
  };

  useEventListener("resize", handleWindowResize, { target: window });

  /**
   * 关闭预览窗口并清理本地预览会话。
   */
  const closePreview = (reason: string) => {
    previewOpenRequestIdRef.current += 1;
    cancelHoverPreview();
    cancelHoverHide();
    cancelPreviewMoveFrame();
    cancelKeyboardPreviewFrame();
    commitPreviewSession(null);
    closeClipboardPreviewSilently(reason, sourceWindowLabel);
  };

  /**
   * A pointerenter alone is not user intent: Windows/WebView can emit it
   * when the window is shown under an unmoving cursor. Space-key previews
   * retain their existing keyboard retargeting behavior.
   */
  const handleItemPointerEnter = (
    item: ClipboardItem,
    _event: ReactPointerEvent<HTMLDivElement>,
  ) => {
    if (!clipboardWindowVisibleRef.current) return;
    if (previewSessionRef.current?.trigger !== "keyboard") return;

    cancelHoverPreview();
    cancelHoverHide();

    if (previewSessionRef.current.itemId === item.id) {
      cancelKeyboardPreviewFrame();
      return;
    }
    scheduleKeyboardPreviewMove(item);
  };

  /**
   * Only a real pointer movement over the actual card arms hover preview.
   * New show sessions start with no recorded position; synthetic stationary
   * moves with unchanged coordinates must not resurrect a previous item.
   */
  const handleItemPointerMove = (
    item: ClipboardItem,
    event: ReactPointerEvent<HTMLDivElement>,
  ) => {
    if (!clipboardWindowVisibleRef.current) return;
    if (event.pointerType !== "mouse" && event.pointerType !== "pen") return;

    const next = { x: event.clientX, y: event.clientY };
    const previous = lastPointerLocationRef.current;
    lastPointerLocationRef.current = next;
    const moved =
      (previous !== null &&
        (previous.x !== next.x || previous.y !== next.y)) ||
      event.nativeEvent.movementX !== 0 ||
      event.nativeEvent.movementY !== 0;
    if (!moved) return;
    if (!pointerIsInsideItem(item.id)) return;

    onHoverSelect(item.id);
    if (previewSessionRef.current?.trigger === "keyboard") return;
    if (!settingsState.clipboard.preview.hoverEnabled) return;

    if (hoverTimerRef.current !== null) {
      if (pendingHoverTargetRef.current?.item.id === item.id) {
        pendingHoverTargetRef.current = { item, pointerY: next.y };
      } else {
        startHoverPreview(item, next.y);
      }
      return;
    }

    if (
      previewSessionRef.current?.trigger === "hover" &&
      previewSessionRef.current.itemId === item.id
    ) {
      previewMoveTargetRef.current = { item, pointerY: next.y };
      if (previewMoveFrameRef.current !== null) return;

      previewMoveFrameRef.current = window.requestAnimationFrame(() => {
        previewMoveFrameRef.current = null;
        const target = previewMoveTargetRef.current;
        previewMoveTargetRef.current = null;
        if (!target || !pointerIsInsideItem(target.item.id)) return;
        if (previewSessionRef.current?.trigger !== "hover") return;
        if (previewSessionRef.current.itemId !== target.item.id) return;
        void openPreviewForItem(target.item, "hover", target.pointerY);
      });
      return;
    }

    startHoverPreview(item, next.y);
  };

  const startHoverPreview = (item: ClipboardItem, pointerY: number) => {
    cancelHoverPreview();
    cancelHoverHide();
    pendingHoverTargetRef.current = { item, pointerY };

    if (previewSessionRef.current?.trigger === "hover") {
      void openPreviewForItem(item, "hover", pointerY);
      return;
    }

    hoverTimerRef.current = window.setTimeout(() => {
      hoverTimerRef.current = null;
      if (!clipboardWindowVisibleRef.current) return;
      if (!settingsState.clipboard.preview.hoverEnabled) return;
      const target = pendingHoverTargetRef.current;
      if (!target || target.item.id !== item.id) return;
      if (!pointerIsInsideItem(item.id)) return;
      void openPreviewForItem(target.item, "hover", target.pointerY);
    }, HOVER_DELAY_MS[previewSettings.hoverDelayMs]);
  };

  /** Confirm the latest observed pointer still hit-tests the same card. */
  function pointerIsInsideItem(itemId: string) {
    const point = lastPointerLocationRef.current;
    const element = itemElementMapRef.current.get(itemId);
    if (!point || !element) return false;
    const rect = element.getBoundingClientRect();
    if (
      point.x < rect.left ||
      point.x >= rect.right ||
      point.y < rect.top ||
      point.y >= rect.bottom
    ) {
      return false;
    }
    const hit = document.elementFromPoint(point.x, point.y);
    return hit !== null && element.contains(hit);
  }

  /**
   * Hover 离开单个卡片时进入准备隐藏状态；进入新卡片会取消隐藏。
   */
  const handleItemPointerLeave = () => {
    scheduleHoverHide("itemPointerLeave");
  };

  /**
   * 指针离开列表区域时结束 hover preview，卡片间移动只做 retarget。
   */
  const handlePreviewAreaPointerLeave = () => {
    lastPointerLocationRef.current = null;
    scheduleHoverHide("hoverAreaLeave");
  };

  /**
   * 指针离开当前 document/window 时兜底关闭，避免 webview 边界漏掉元素级 leave。
   */
  const handleDocumentPointerOut = (event: PointerEvent) => {
    if (event.relatedTarget !== null) return;

    lastPointerLocationRef.current = null;
    scheduleHoverHide("documentPointerOut");
  };

  useEventListener("pointerout", handleDocumentPointerOut, {
    target: document,
  });

  /**
   * 浏览器取消后续指针事件时关闭 hover preview，避免预览残留。
   */
  const handleDocumentPointerCancel = () => {
    lastPointerLocationRef.current = null;
    scheduleHoverHide("documentPointerCancel");
  };

  useEventListener("pointercancel", handleDocumentPointerCancel, {
    target: document,
  });

  /**
   * Space 按下打开当前 active item；忽略重复 keydown，避免重复 IPC。
   */
  const handlePreviewSpaceDown = (event: KeyboardEvent) => {
    event.preventDefault();

    if (!previewSettings.spaceEnabled) return;
    if (event.repeat && previewSession?.trigger === "keyboard") return;

    cancelHoverPreview();
    cancelHoverHide();

    const activeItem = getActiveItem();

    if (!activeItem) return;

    void openPreviewForItem(activeItem, "keyboard");
  };

  /**
   * Space 松开关闭 keyboard preview。
   */
  const handlePreviewSpaceUp = (event: KeyboardEvent) => {
    if (!isSpaceKey(event)) return;

    event.preventDefault();

    if (previewSession?.trigger !== "keyboard") return;

    closePreview("spaceUp");
  };

  useKeyboardEvent("keyup", handlePreviewSpaceUp);

  /**
   * 方向键移动到新 active item 时同步 keyboard preview。
   */
  const handleKeyboardPreviewMove = (item: ClipboardItem) => {
    if (previewSessionRef.current?.trigger !== "keyboard") return;

    scheduleKeyboardPreviewMove(item);
  };

  /**
   * 取消等待中的 hover preview。
   */
  const cancelHoverPreview = () => {
    clearHoverTimer(hoverTimerRef);
    pendingHoverTargetRef.current = null;
  };

  /**
   * 滚动列表时关闭 hover preview，保留 keyboard preview。
   */
  const closeHoverPreviewForScroll = () => {
    if (previewSession?.trigger === "hover") {
      closePreview("scroll");
      return;
    }

    cancelHoverPreview();
  };

  /**
   * 打开或重定向指定条目的预览 overlay。
   */
  const openPreviewForItem = async (
    item: ClipboardItem,
    trigger: PreviewTrigger,
    pointerY?: number,
  ) => {
    if (!clipboardWindowVisibleRef.current) return;
    if (trigger === "hover" && !pointerIsInsideItem(item.id)) return;
    if (
      !canStartPreviewOpen(
        item.id,
        previewOpenInFlightRef.current?.itemId ?? null,
        previewOpenRetryAfterRef.current,
        Date.now(),
      )
    ) {
      return;
    }

    const element = itemElementMapRef.current.get(item.id);
    const rect = element?.getBoundingClientRect();

    if (!rect || rect.width <= 0 || rect.height <= 0) return;

    const requestId = previewOpenRequestIdRef.current + 1;
    previewOpenRequestIdRef.current = requestId;
    previewOpenInFlightRef.current = { itemId: item.id, requestId };
    commitPreviewSession({ itemId: item.id, trigger });

    try {
      if (!clipboardWindowVisibleRef.current) return;

      const state = await showClipboardPreview(
        item.id,
        {
          height: rect.height,
          left: rect.left,
          pointerY,
          top: rect.top,
          width: rect.width,
        },
        sourceWindowLabel,
      );

      if (requestId !== previewOpenRequestIdRef.current) return;
      if (!state || !clipboardWindowVisibleRef.current) {
        closePreview("previewShowSuppressed");
      }
    } catch (error) {
      if (requestId === previewOpenRequestIdRef.current) {
        previewOpenRetryAfterRef.current =
          Date.now() + PREVIEW_OPEN_FAILURE_COOLDOWN_MS;
        log.error("show clipboard preview failed", { error, trigger });
        commitPreviewSession(null);
      }
      return;
    } finally {
      if (previewOpenInFlightRef.current?.requestId === requestId) {
        previewOpenInFlightRef.current = null;
      }
    }
  };

  /**
   * 取消等待中的 hover 隐藏缓冲。
   */
  const cancelHoverHide = () => {
    clearHoverTimer(hoverHideTimerRef);
  };

  /**
   * 结束 hover preview；keyboard preview 不受指针离开影响。
   */
  const closeHoverPreview = (reason: string) => {
    cancelHoverPreview();
    cancelHoverHide();

    if (previewSessionRef.current?.trigger !== "hover") return;

    closePreview(reason);
  };

  /**
   * 鼠标离开剪贴板项后进入准备隐藏状态，短时间内进入新项会取消隐藏。
   */
  const scheduleHoverHide = (reason: string) => {
    cancelHoverPreview();
    cancelHoverHide();

    if (previewSessionRef.current?.trigger !== "hover") return;

    hoverHideTimerRef.current = window.setTimeout(() => {
      hoverHideTimerRef.current = null;
      closeHoverPreview(reason);
    }, HOVER_HIDE_BUFFER_MS);
  };

  /**
   * 取消等待中的 mousemove retarget 帧。
   */
  function cancelPreviewMoveFrame() {
    if (previewMoveFrameRef.current === null) return;

    window.cancelAnimationFrame(previewMoveFrameRef.current);
    previewMoveFrameRef.current = null;
    previewMoveTargetRef.current = null;
    pendingHoverTargetRef.current = null;
  }

  /**
   * 方向键会先触发虚拟列表滚动，等待目标卡片位置稳定后再重定向预览。
   */
  function scheduleKeyboardPreviewMove(item: ClipboardItem) {
    cancelKeyboardPreviewFrame();
    keyboardPreviewTargetRef.current = {
      frames: 0,
      item,
      lastRect: null,
      stableFrames: 0,
    };
    requestKeyboardPreviewFrame();
  }

  /**
   * 请求下一帧键盘预览位置采样。
   */
  function requestKeyboardPreviewFrame() {
    keyboardPreviewFrameRef.current = window.requestAnimationFrame(
      handleKeyboardPreviewFrame,
    );
  }

  /**
   * 等待目标卡片 DOMRect 在滚动后稳定，再用最新位置打开预览。
   */
  function handleKeyboardPreviewFrame() {
    keyboardPreviewFrameRef.current = null;

    const target = keyboardPreviewTargetRef.current;

    if (!target) return;

    if (previewSessionRef.current?.trigger !== "keyboard") {
      keyboardPreviewTargetRef.current = null;
      return;
    }

    target.frames += 1;

    const rect = resolveItemRect(target.item.id);

    if (!rect) {
      retryKeyboardPreviewFrame(target);
      return;
    }

    if (
      target.lastRect &&
      isPreviewRectStable(target.lastRect, rect, KEYBOARD_PREVIEW_RECT_EPSILON)
    ) {
      target.stableFrames += 1;
    } else {
      target.stableFrames = 0;
    }

    target.lastRect = rect;

    if (
      target.stableFrames >= KEYBOARD_PREVIEW_STABLE_FRAMES ||
      target.frames >= KEYBOARD_PREVIEW_MAX_FRAMES
    ) {
      keyboardPreviewTargetRef.current = null;
      void openPreviewForItem(target.item, "keyboard");
      return;
    }

    requestKeyboardPreviewFrame();
  }

  /**
   * 目标卡片尚未挂载或滚动仍在进行时继续等待，超过上限后放弃旧目标。
   */
  function retryKeyboardPreviewFrame(target: KeyboardPreviewTarget) {
    if (target.frames >= KEYBOARD_PREVIEW_MAX_FRAMES) {
      keyboardPreviewTargetRef.current = null;
      return;
    }

    requestKeyboardPreviewFrame();
  }

  /**
   * 取消等待中的键盘预览位置采样。
   */
  function cancelKeyboardPreviewFrame() {
    if (keyboardPreviewFrameRef.current !== null) {
      window.cancelAnimationFrame(keyboardPreviewFrameRef.current);
      keyboardPreviewFrameRef.current = null;
    }

    keyboardPreviewTargetRef.current = null;
  }

  /**
   * 读取列表项当前 DOMRect，并转成可比较的普通对象。
   */
  function resolveItemRect(id: string): PreviewRectSnapshot | null {
    const element = itemElementMapRef.current.get(id);
    const rect = element?.getBoundingClientRect();

    if (!rect || rect.width <= 0 || rect.height <= 0) return null;

    return {
      height: rect.height,
      left: rect.left,
      top: rect.top,
      width: rect.width,
    };
  }

  /**
   * 同步预览会话 state 与 ref，供 hover 定时器和快速切换时读取最新会话。
   */
  function commitPreviewSession(session: PreviewSession | null) {
    previewSessionRef.current = session;
    setPreviewSession(session);
  }

  return {
    closeHoverPreviewForScroll,
    closePreview,
    handleItemPointerEnter,
    handleItemPointerLeave,
    handleItemPointerMove,
    handleKeyboardPreviewMove,
    handlePreviewAreaPointerLeave,
    handlePreviewSpaceDown,
    previewSession,
  };
}

/**
 * 判断两帧 DOMRect 是否已经稳定，避免 smooth scroll 中途采样旧坐标。
 */
function isPreviewRectStable(
  prev: PreviewRectSnapshot,
  next: PreviewRectSnapshot,
  epsilon: number,
) {
  return (
    Math.abs(prev.left - next.left) <= epsilon &&
    Math.abs(prev.top - next.top) <= epsilon &&
    Math.abs(prev.width - next.width) <= epsilon &&
    Math.abs(prev.height - next.height) <= epsilon
  );
}

/**
 * 判断 Space 键。
 */
export { isSpaceKey } from "./previewController";
