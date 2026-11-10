import { listen } from "@tauri-apps/api/event";
import {
  getSettings,
  resetSettings as invokeResetSettings,
  updateSettings as invokeUpdateSettings,
} from "@/commands";
import { TAURI_EVENT } from "@/constants/events";
import type { Settings, SettingsPatch } from "@/types/settings";
import { log } from "@/utils/log";
import { settingsState } from "./settingsState";

export { settingsState };

/**
 * 设置的本地镜像，真相源在 Rust（`SettingsStore`）。
 *
 * 数据流向是**严格单向**的：
 *   组件 → `updateSettings(patch)` → Rust 落盘 → Rust `emit("settings://updated")`
 *   → 全部窗口的 `listen` → `Object.assign(settingsState, payload)` → 组件重渲染
 *
 * 因此 `updateSettings` 本身不修改 `settingsState`——发起方也通过事件回灌，保证
 * 多窗口取到的镜像永远等于 Rust 的最新快照，避免本地乐观更新引发漂移。
 *
 * 字面量初值仅为占位；组件层通过 `use(settingsReady)` 挂起到首屏快照灌入后才会读取。
 */

/**
 * 启动期一次性初始化：订阅 Rust 广播 + 拉取首屏快照。
 * 模块导入即开跑，由 React `use(settingsReady)` 在 Suspense 中等待完成。
 * 每个 webview 加载本模块一次，因此事件订阅天然单例。
 */
/**
 * Loading the initial settings must not be blocked by an event-listener registration.
 * A hidden WebView may delay registration; the main UI still needs its first snapshot.
 */
const showBootstrapStage = (stage: string) => {
  const status = document.getElementById("ecopaste-main-boot");
  const details = document.getElementById("ecopaste-boot-details");
  if (status?.style.display === "flex" && details) {
    details.textContent = stage;
  }
};

/**
 * The main WebView is created hidden. Avoid sending startup IPC while its first
 * frame is still suspended. Other windows keep their existing startup path.
 */
const firstSafeFrame = (): Promise<void> => {
  const hash = window.location.hash;
  if (hash !== "" && hash !== "#" && hash !== "#/") {
    return Promise.resolve();
  }

  showBootstrapStage("等待主窗口首次绘制后读取设置。");
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => {
      resolve();
    });
  });
};

const SETTINGS_IPC_TIMEOUT_MS = 4_500;
const SETTINGS_IPC_MAX_ATTEMPTS = 3;

/**
 * A startup IPC may be sent before Tauri's managed SettingsStore is ready.
 * Re-issuing this read-only command is safe. Never leave the Suspense gate
 * pending forever: a bounded failure becomes a visible bootstrap error.
 */
const readInitialSettings = async (): Promise<Settings> => {
  let lastFailure: unknown;

  for (let attempt = 1; attempt <= SETTINGS_IPC_MAX_ATTEMPTS; attempt++) {
    showBootstrapStage(
      `正在读取本机设置（第 ${attempt}/${SETTINGS_IPC_MAX_ATTEMPTS} 次）。`,
    );

    let timeout: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        getSettings(),
        new Promise<Settings>((_resolve, reject) => {
          timeout = setTimeout(() => {
            reject(new Error("get_settings IPC exceeded 4.5 seconds"));
          }, SETTINGS_IPC_TIMEOUT_MS);
        }),
      ]);
    } catch (error) {
      lastFailure = error;
      log.warn("initial settings IPC attempt failed", {
        attempt,
        reason: String(error),
      });
    } finally {
      if (timeout !== undefined) clearTimeout(timeout);
    }
  }

  throw lastFailure ?? new Error("initial settings IPC unavailable");
};

export const settingsReady: Promise<void> = (async () => {
  let initialLoaded = false;
  let bufferedUpdate: Settings | null = null;
  let subscriptionReady = false;

  // Hidden main WebViews must wait for the first browser frame before IPC.
  // Both snapshot retrieval and subscription start only after that point.
  const frameReady = firstSafeFrame();
  const initialRequest = frameReady.then(() => readInitialSettings());

  // Register in parallel; buffer early updates and resync if registration is late.
  void frameReady
    .then(() =>
      listen<Settings>(TAURI_EVENT.SETTINGS_UPDATED, (event) => {
        if (!initialLoaded) {
          bufferedUpdate = event.payload;
          return;
        }
        Object.assign(settingsState, event.payload);
      }),
    )
    .then(() => {
      subscriptionReady = true;
      if (initialLoaded) {
        // Close the subscribe/snapshot race if registration completed late.
        void getSettings()
          .then((latest) => {
            Object.assign(settingsState, latest);
          })
          .catch((error) => {
            log.error("settings post-subscribe resync failed", error);
          });
      }
    })
    .catch((error) => {
      log.error("settings updates listener registration failed", error);
    });

  try {
    const initial = await initialRequest;
    Object.assign(settingsState, initial);
    initialLoaded = true;

    if (bufferedUpdate) {
      Object.assign(settingsState, bufferedUpdate);
      bufferedUpdate = null;
    }
    showBootstrapStage("本机设置读取完成，正在渲染剪贴板主界面。");
    log.info("settings initial snapshot ready", { subscriptionReady });
  } catch (error) {
    showBootstrapStage(`读取本机设置失败：${String(error)}`);
    log.error("settings initial snapshot failed", error);
    throw error;
  }
})();

/**
 * 提交设置补丁；不在此处更新镜像，等 Rust 广播 `settings://updated` 后由 listen 统一回灌。
 * 返回的快照仅为调用方需要立即拿到结果时使用（如表单关闭前校验）。
 */
export async function updateSettings(patch: SettingsPatch): Promise<Settings> {
  return invokeUpdateSettings(patch);
}

/**
 * 恢复所有设置默认值；镜像仍等待 Rust 广播统一回灌。
 */
export async function resetSettings(): Promise<Settings> {
  return invokeResetSettings();
}
