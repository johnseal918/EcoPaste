import type {
  ClipboardGroupIcon,
  ClipboardItemQuery,
  ClipboardKind,
} from "@/types/clipboard";
import type { SidePanelKind } from "@/types/settings";

export interface SidePanelDefinition {
  icon: ClipboardGroupIcon;
  kind: SidePanelKind;
  labelKey: string;
}

export const SIDE_PANEL_DEFINITIONS: SidePanelDefinition[] = [
  {
    icon: "i-ph:push-pin-bold",
    kind: "pinned",
    labelKey: "sidePanels.pinned",
  },
  {
    icon: "i-lets-icons:star",
    kind: "favorite",
    labelKey: "sidePanels.favorite",
  },
  {
    icon: "i-lets-icons:file-dock",
    kind: "text",
    labelKey: "sidePanels.text",
  },
  {
    icon: "i-lets-icons:img-box",
    kind: "image",
    labelKey: "sidePanels.image",
  },
  {
    icon: "i-lets-icons:folder-file-alt",
    kind: "files",
    labelKey: "sidePanels.files",
  },
];

export const SIDE_PANEL_KINDS = SIDE_PANEL_DEFINITIONS.map(({ kind }) => kind);

/**
 * Left/right panel queries share the same contract. Pin stays manually ordered;
 * favorites and content categories are retrieval views sorted by most recent use.
 */
export function sidePanelQuery(kind: SidePanelKind): ClipboardItemQuery {
  const base: ClipboardItemQuery = { sort: "updatedAtDesc" };
  if (kind === "pinned") return { ...base, pinned: true };
  const recent: ClipboardItemQuery = { ...base, recentFirst: true };
  if (kind === "favorite") return { ...recent, favorite: true };
  return { ...recent, kind: kind as ClipboardKind };
}
