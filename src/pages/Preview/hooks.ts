import { useEffect, useRef, useState } from "react";
import { useSnapshot } from "valtio";
import {
  type ClipboardPreviewPayload,
  type ClipboardPreviewState,
  getClipboardPreviewPayload,
} from "@/commands";
import { TAURI_EVENT } from "@/constants/events";
import { useTauriListen } from "@/hooks/useTauriListen";
import { settingsState } from "@/stores/settings";
import { log } from "@/utils/log";
import {
  beginPreviewCacheSession,
  invalidateCachedPayload,
  readCachedPayload,
  writeCachedPayload,
} from "./cache";
import { PREVIEW_EXIT_ANIMATION_MS } from "./constants";

/**
 * 保留退出动画期间的最后一帧 preview state，动画结束后再清空渲染树。
 */
export function usePreviewRenderState(
  previewState: ClipboardPreviewState | null,
) {
  const [renderState, setRenderState] = useState<ClipboardPreviewState | null>(
    null,
  );
  const exitTimerRef = useRef<number | null>(null);

  useEffect(() => {
    if (exitTimerRef.current !== null) {
      window.clearTimeout(exitTimerRef.current);
      exitTimerRef.current = null;
    }

    if (previewState) {
      setRenderState(previewState);
      return;
    }

    exitTimerRef.current = window.setTimeout(() => {
      setRenderState(null);
      exitTimerRef.current = null;
    }, PREVIEW_EXIT_ANIMATION_MS);

    return () => {
      if (exitTimerRef.current === null) return;

      window.clearTimeout(exitTimerRef.current);
      exitTimerRef.current = null;
    };
  }, [previewState]);

  return renderState;
}

/**
 * 按预览状态加载 payload，并用 LRU cache 复用最近内容。
 */
export function usePreviewPayload(
  previewState: ClipboardPreviewState | null,
  resetToken = 0,
) {
  const [payload, setPayload] = useState<ClipboardPreviewPayload | null>(null);
  const [loadingItemId, setLoadingItemId] = useState<string | null>(null);
  const latestRequestIdRef = useRef(0);
  const cacheRef = useRef(new Map<string, ClipboardPreviewPayload>());
  const cacheSessionIdRef = useRef<number | null>(null);
  const activePreviewRef = useRef(previewState);
  activePreviewRef.current = previewState;
  const [invalidationSerial, setInvalidationSerial] = useState(0);
  const { clipboard } = useSnapshot(settingsState);
  const redactSecrets = clipboard.sensitive.redactSecrets;

  // 后端只在数据库提交后广播 clipboard://updated。
  // 编辑时立即让对应 id 的旧预览失效；若正在展示该条，主动重新读取。
  useTauriListen<{ id?: string }>(TAURI_EVENT.CLIPBOARD_UPDATED, (event) => {
    const id = event.payload.id;
    if (id) {
      invalidateCachedPayload(cacheRef.current, id);
    } else {
      cacheRef.current.clear();
    }

    const current = activePreviewRef.current;
    if (current && (!id || current.itemId === id)) {
      setPayload(null);
      setLoadingItemId(current.itemId);
      setInvalidationSerial((value) => value + 1);
    }
  });

  // biome-ignore lint/correctness/useExhaustiveDependencies: resetToken 是销毁前清缓存的纯触发器。
  useEffect(() => {
    cacheRef.current.clear();
    cacheSessionIdRef.current = null;
    latestRequestIdRef.current += 1;
    setLoadingItemId(null);
    setPayload(null);
  }, [resetToken]);

  useEffect(() => {
    if (!previewState) {
      // 保留退出动画的最后一帧，但不保留可供下次打开直接命中的旧内容缓存。
      cacheRef.current.clear();
      cacheSessionIdRef.current = null;
      setLoadingItemId(null);
      return;
    }

    const state = previewState;
    const newSession = cacheSessionIdRef.current !== state.sessionId;
    cacheSessionIdRef.current = beginPreviewCacheSession(
      cacheRef.current,
      cacheSessionIdRef.current,
      state.sessionId,
    );
    if (newSession) setPayload(null);

    let cancelled = false;
    latestRequestIdRef.current = state.requestId;

    const cached = readCachedPayload(
      cacheRef.current,
      state.itemId,
      redactSecrets,
    );
    if (cached) {
      setPayload(cached);
      setLoadingItemId(null);
      return;
    }

    setLoadingItemId(state.itemId);

    async function loadPayload() {
      try {
        const nextPayload = await getClipboardPreviewPayload(state.itemId);

        if (cancelled || latestRequestIdRef.current !== state.requestId) {
          return;
        }

        if (!nextPayload) {
          setPayload(null);
          setLoadingItemId(null);
          return;
        }

        writeCachedPayload(cacheRef.current, nextPayload, redactSecrets);
        setPayload(nextPayload);
        setLoadingItemId(null);
      } catch (error) {
        if (cancelled || latestRequestIdRef.current !== state.requestId) {
          return;
        }

        log.error("load preview payload failed", error);
        setLoadingItemId(null);
      }
    }

    void loadPayload();

    return () => {
      cancelled = true;
    };
  }, [previewState, redactSecrets, invalidationSerial]);

  return { loadingItemId, payload };
}
