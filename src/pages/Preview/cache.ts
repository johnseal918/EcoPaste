import type { ClipboardPreviewPayload } from "@/commands";
import { PREVIEW_CACHE_LIMIT } from "./constants";

/**
 * 从 LRU cache 读取同 item 的最新 payload，命中后刷新插入顺序。
 */
export function readCachedPayload(
  cache: Map<string, ClipboardPreviewPayload>,
  itemId: string,
  redactSecrets: boolean,
) {
  const keyPrefix = `${itemId}:`;
  const keySuffix = `:${redactSecrets ? "redacted" : "full"}`;
  // 同一条目可能曾缓存多个版本，应优先采用最后插入的版本。
  const key = [...cache.keys()].reverse().find((entryKey) => {
    return entryKey.startsWith(keyPrefix) && entryKey.endsWith(keySuffix);
  });

  if (!key) return null;

  const cached = cache.get(key) ?? null;
  if (cached) {
    cache.delete(key);
    cache.set(key, cached);
  }

  return cached;
}

/**
 * 记录正文修改/删除/导入后按 id 清除所有历史版本及脱敏变体。
 */
export function invalidateCachedPayload(
  cache: Map<string, ClipboardPreviewPayload>,
  itemId: string,
) {
  const prefix = `${itemId}:`;
  for (const key of cache.keys()) {
    if (key.startsWith(prefix)) cache.delete(key);
  }
}

/**
 * 同一次连续悬停可复用缓存。新预览会话必须重新读取数据库最新正文；
 * 否则按 id 命中旧缓存，即使该记录 updatedAt 已变，仍会显示旧文本。
 */
export function beginPreviewCacheSession(
  cache: Map<string, ClipboardPreviewPayload>,
  previousSessionId: number | null,
  nextSessionId: number,
): number {
  if (previousSessionId !== nextSessionId) cache.clear();
  return nextSessionId;
}

/**
 * 写入最近预览 payload，key 绑定 updatedAt 避免内容复用过期。
 */
export function writeCachedPayload(
  cache: Map<string, ClipboardPreviewPayload>,
  nextPayload: ClipboardPreviewPayload,
  redactSecrets: boolean,
) {
  // 新版本进入缓存时丢弃本 id 以前的版本，避免同条目命中较早的正文。
  invalidateCachedPayload(cache, nextPayload.id);
  cache.set(cacheKey(nextPayload, redactSecrets), nextPayload);

  while (cache.size > PREVIEW_CACHE_LIMIT) {
    const [oldestKey] = cache.keys();
    if (!oldestKey) return;

    cache.delete(oldestKey);
  }
}

/**
 * 生成预览 payload 的缓存 key。
 */
export function cacheKey(
  payload: ClipboardPreviewPayload,
  redactSecrets = payload.isSensitive,
) {
  return `${payload.id}:${payload.updatedAt}:${redactSecrets ? "redacted" : "full"}`;
}
