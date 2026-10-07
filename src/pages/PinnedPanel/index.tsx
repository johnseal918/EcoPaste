import type { MenuProps } from "antd";
import { Dropdown, Empty, Spin, Switch } from "antd";
import type { FC, MouseEvent as ReactMouseEvent } from "react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Virtuoso } from "react-virtuoso";
import { useSnapshot } from "valtio";
import {
  getClipboardSidePanelsState,
  movePinnedClipboardItem,
  pasteClipboardItem,
  setClipboardSidePanelOpen,
  toggleClipboardItemPinned,
} from "@/commands";
import ClipboardGroupIcon from "@/components/ClipboardGroupIcon";
import Tooltip from "@/components/Tooltip";
import { TAURI_EVENT } from "@/constants/events";
import { SIDE_PANEL_DEFINITIONS, SIDE_PANEL_KINDS } from "@/constants/sidePanels";
import { useClipboardItems } from "@/hooks/useClipboardItems";
import { useClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";
import { useTauriListen } from "@/hooks/useTauriListen";
import { settingsState, updateSettings } from "@/stores/settings";
import type {
  ClipboardItem,
  ClipboardItemQuery,
  ClipboardKind,
} from "@/types/clipboard";
import type { SidePanelKind } from "@/types/settings";
import ClipboardCard from "../Clipboard/components/cards/ClipboardCard";
import OrderPositionModal from "../Clipboard/components/OrderPositionModal";

interface ClipboardUpdatedPayload {
  metadata?: boolean;
}

interface ClipboardSidePanelsRuntimeState {
  open: SidePanelKind[];
}

interface SidePanelColumnProps {
  canMoveLeft: boolean;
  canMoveRight: boolean;
  kind: SidePanelKind;
  onClose: (kind: SidePanelKind) => void;
  onMove: (kind: SidePanelKind, direction: -1 | 1) => void;
  onToggleAlwaysShow: (kind: SidePanelKind, enabled: boolean) => void;
}

const PinnedPanel: FC = () => {
  useClipboardWindowEditableFocus();

  const settings = useSnapshot(settingsState);
  const [openPanels, setOpenPanels] = useState<SidePanelKind[]>([]);

  useEffect(() => {
    void getClipboardSidePanelsState().then((state) => {
      setOpenPanels(state.open);
    });
  }, []);

  useTauriListen<ClipboardSidePanelsRuntimeState>(
    TAURI_EVENT.CLIPBOARD_SIDE_PANELS_UPDATED,
    (event) => {
      setOpenPanels(event.payload.open);
    },
  );

  const orderedOpenPanels = useMemo(() => {
    return normalizePanelOrder(
      settings.clipboard.sidePanels.order as SidePanelKind[],
      openPanels,
    );
  }, [openPanels, settings.clipboard.sidePanels.order]);

  const handleClose = (kind: SidePanelKind) => {
    void setClipboardSidePanelOpen(kind, false).then((state) => {
      setOpenPanels(state.open);
    });
  };

  const handleToggleAlwaysShow = (
    kind: SidePanelKind,
    enabled: boolean,
  ) => {
    const current = [
      ...(settings.clipboard.sidePanels.alwaysShow as SidePanelKind[]),
    ];
    const next = enabled
      ? Array.from(new Set([...current, kind]))
      : current.filter((candidate) => candidate !== kind);

    void updateSettings({
      clipboard: {
        sidePanels: {
          alwaysShow: next,
        },
      },
    });
  };

  const handleMove = (kind: SidePanelKind, direction: -1 | 1) => {
    const visible = orderedOpenPanels;
    const visibleIndex = visible.indexOf(kind);
    const targetVisible = visible[visibleIndex + direction];
    if (!targetVisible) return;

    const currentOrder = normalizePanelOrder(
      settings.clipboard.sidePanels.order as SidePanelKind[],
      SIDE_PANEL_KINDS,
    );
    const from = currentOrder.indexOf(kind);
    const to = currentOrder.indexOf(targetVisible);
    if (from < 0 || to < 0) return;

    [currentOrder[from], currentOrder[to]] = [
      currentOrder[to],
      currentOrder[from],
    ];

    void updateSettings({
      clipboard: {
        sidePanels: {
          order: currentOrder,
        },
      },
    });
  };

  return (
    <div className="flex size-screen overflow-hidden bg-ant-container">
      {orderedOpenPanels.map((kind, index) => {
        return (
          <div
            className="min-w-0 flex-1 border-ant-border border-l first:border-l-0"
            key={kind}
          >
            <SidePanelColumn
              canMoveLeft={index > 0}
              canMoveRight={index < orderedOpenPanels.length - 1}
              kind={kind}
              onClose={handleClose}
              onMove={handleMove}
              onToggleAlwaysShow={handleToggleAlwaysShow}
            />
          </div>
        );
      })}
    </div>
  );
};

const SidePanelColumn: FC<SidePanelColumnProps> = (props) => {
  const {
    canMoveLeft,
    canMoveRight,
    kind,
    onClose,
    onMove,
    onToggleAlwaysShow,
  } = props;
  const { t } = useTranslation("clipboard");
  const settings = useSnapshot(settingsState);
  const [orderTarget, setOrderTarget] = useState<ClipboardItem | null>(null);
  const query = panelQuery(kind);
  const { getItem, loadedInitial, loading, loadRange, reload, total } =
    useClipboardItems(query);

  useTauriListen<ClipboardUpdatedPayload>(TAURI_EVENT.CLIPBOARD_UPDATED, () => {
    reload();
  });

  const definition =
    SIDE_PANEL_DEFINITIONS.find((candidate) => candidate.kind === kind) ??
    SIDE_PANEL_DEFINITIONS[0];
  const alwaysShow = (
    settings.clipboard.sidePanels.alwaysShow as SidePanelKind[]
  ).includes(kind);
  const rowCount = Math.ceil(total / 2);

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden bg-ant-container">
      <div className="flex h-12 shrink-0 items-center gap-2 border-ant-border border-b px-2">
        <ClipboardGroupIcon icon={definition.icon} />
        <span className="min-w-0 truncate font-medium">
          {t(definition.labelKey)}
        </span>
        <span className="text-ant-secondary text-xs">{total}</span>

        <div className="ml-auto flex shrink-0 items-center gap-1">
          <Tooltip title={t("sidePanels.alwaysShow")}>
            <div className="flex items-center gap-1">
              <span className="whitespace-nowrap text-ant-secondary text-xs">
                {t("sidePanels.alwaysShow")}
              </span>
              <Switch
                checked={alwaysShow}
                onChange={(checked) => {
                  onToggleAlwaysShow(kind, checked);
                }}
                size="small"
              />
            </div>
          </Tooltip>

          <PanelHeaderButton
            disabled={!canMoveLeft}
            icon="i-lucide:chevron-left"
            label={t("sidePanels.moveLeft")}
            onClick={() => {
              onMove(kind, -1);
            }}
          />
          <PanelHeaderButton
            disabled={!canMoveRight}
            icon="i-lucide:chevron-right"
            label={t("sidePanels.moveRight")}
            onClick={() => {
              onMove(kind, 1);
            }}
          />
          <PanelHeaderButton
            icon="i-lucide:x"
            label={t("sidePanels.close")}
            onClick={() => {
              onClose(kind);
            }}
          />
        </div>
      </div>

      {loading && !loadedInitial ? (
        <div className="flex flex-1 items-center justify-center">
          <Spin />
        </div>
      ) : loadedInitial && total === 0 ? (
        <div className="flex flex-1 items-center justify-center px-4">
          <Empty
            description={t(`sidePanels.empty.${kind}`)}
            image={Empty.PRESENTED_IMAGE_SIMPLE}
          />
        </div>
      ) : (
        <Virtuoso
          className="flex-1"
          itemContent={(rowIndex) => {
            const left = getItem(rowIndex * 2);
            const right = getItem(rowIndex * 2 + 1);

            return (
              <div className="grid grid-cols-2 gap-2 px-2 pt-2">
                {left ? renderCard(left) : <div />}
                {right ? renderCard(right) : <div />}
              </div>
            );
          }}
          rangeChanged={({ startIndex, endIndex }) => {
            loadRange(startIndex * 2, endIndex * 2 + 1);
          }}
          totalCount={rowCount}
        />
      )}

      {kind === "pinned" ? (
        <OrderPositionModal
          currentPosition={orderTarget?.pinOrder}
          onCancel={() => {
            setOrderTarget(null);
          }}
          onConfirm={async (position) => {
            if (!orderTarget) return;

            await movePinnedClipboardItem(orderTarget.id, position);
            setOrderTarget(null);
            reload();
          }}
          open={orderTarget !== null}
        />
      ) : null}
    </div>
  );

  function renderCard(item: ClipboardItem) {
    const handleMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;

      void pasteClipboardItem(item.id, false);
    };

    if (kind !== "pinned") {
      return (
        <ClipboardCard
          isSelected={false}
          item={item}
          onMouseDown={handleMouseDown}
          quickActions={[]}
          showOriginalOnHover={false}
        />
      );
    }

    const menu: MenuProps = {
      items: [
        { key: "first", label: t("pinnedPanel.moveFirst") },
        { key: "last", label: t("pinnedPanel.moveLast") },
        { key: "position", label: t("pinnedPanel.moveTo") },
        { type: "divider" },
        { key: "unpin", label: t("pinnedPanel.unpin") },
      ],
      onClick: async ({ key, domEvent }) => {
        domEvent.stopPropagation();

        if (key === "first") {
          await movePinnedClipboardItem(item.id, 1);
          reload();
          return;
        }
        if (key === "last") {
          await movePinnedClipboardItem(item.id, Number.MAX_SAFE_INTEGER);
          reload();
          return;
        }
        if (key === "position") {
          setOrderTarget(item);
          return;
        }
        if (key === "unpin") {
          await toggleClipboardItemPinned(item.id, false);
          reload();
        }
      },
    };

    return (
      <Dropdown menu={menu} trigger={["contextMenu"]}>
        <div>
          <ClipboardCard
            disableContextMenu
            isSelected={false}
            item={item}
            onMouseDown={handleMouseDown}
            quickActions={[]}
            showOriginalOnHover={false}
          />
        </div>
      </Dropdown>
    );
  }
};

const PanelHeaderButton: FC<{
  disabled?: boolean;
  icon: string;
  label: string;
  onClick: () => void;
}> = ({ disabled = false, icon, label, onClick }) => {
  return (
    <Tooltip title={label}>
      <button
        aria-label={label}
        className="flex size-6 items-center justify-center rounded-1 border-0 bg-transparent p-0 text-ant-secondary enabled:hover:bg-ant-fill-tertiary disabled:cursor-default disabled:opacity-30"
        disabled={disabled}
        onClick={onClick}
        type="button"
      >
        <i aria-hidden className={icon} />
      </button>
    </Tooltip>
  );
};

function panelQuery(kind: SidePanelKind): ClipboardItemQuery {
  const base: ClipboardItemQuery = { sort: "updatedAtDesc" };

  if (kind === "pinned") return { ...base, pinned: true };
  if (kind === "favorite") return { ...base, favorite: true };

  return { ...base, kind: kind as ClipboardKind };
}

function normalizePanelOrder(
  preferred: SidePanelKind[],
  selected: SidePanelKind[],
) {
  const result: SidePanelKind[] = [];

  for (const kind of preferred) {
    if (selected.includes(kind) && !result.includes(kind)) result.push(kind);
  }
  for (const kind of SIDE_PANEL_KINDS) {
    if (selected.includes(kind) && !result.includes(kind)) result.push(kind);
  }

  return result;
}

export default PinnedPanel;
