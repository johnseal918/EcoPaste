import type { ClipboardGroupIcon } from "@/types/clipboard";
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
