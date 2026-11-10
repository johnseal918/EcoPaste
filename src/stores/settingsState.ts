import { proxy } from "valtio";
import type { Settings } from "@/types/settings";

/**
 * One shared settings proxy per WebView. This module intentionally has no
 * dependency on commands: the command module imports this store for deletion
 * safeguards, while settings bootstrap imports commands for IPC.
 * Keeping the proxy separate breaks the ES-module initialization cycle.
 */
export const settingsState = proxy<Settings>({} as Settings);
