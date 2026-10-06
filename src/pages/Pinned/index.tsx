import { Empty, Spin } from "antd";
import type { ComponentProps, FC, MouseEvent } from "react";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { VirtuosoGrid } from "react-virtuoso";
import { useSnapshot } from "valtio";
import {
  deleteClipboardItem,
  movePinnedClipboardItemOrder,
  openClipboardItemLink,
  pasteClipboardItem,
  revealClipboardItem,
  saveClipboardImageToFile,
  toggleClipboardItemFavorite,
  toggleClipboardItemPinned,
  updateClipboardItemGroup,
  writeToClipboard,
} from "@/commands";
import { TAURI_EVENT } from "@/constants/events";
import { WINDOW_LABEL } from "@/constants/windows";
import { useClipboardItems } from "@/hooks/useClipboardItems";
import { useClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";
import { useTauriListen } from "@/hooks/useTauriListen";
import { settingsState } from "@/stores/settings";
import type { ClipboardAction, ClipboardItem } from "@/types/clipboard";
import { cn } from "@/utils/cn";
import ClipboardCard from "../Clipboard/components/cards/ClipboardCard";
import NoteModal from "../Clipboard/components/NoteModal";
import OrderPositionModal from "../Clipboard/components/OrderPositionModal";

interface ClipboardMenuActionPayload {
  action: ClipboardAction;
  groupId?: string;
  itemId: string;
}

interface WindowVisibilityPayload {
  label: string;
  visible: boolean;
}

const GridList: FC<ComponentProps<"div">> = (props) => {
  const { className, ...rest } = props;

  return <div className={cn("flex flex-wrap p-2", className)} {...rest} />;
};

const GridItem: FC<ComponentProps<"div">> = (props) => {
  const { className, ...rest } = props;

  return <div className={cn("w-1/2 p-1", className)} {...rest} />;
};

/**
 * 独立右侧置顶面板：仅展示 pinned 条目，按 pinOrder 两列排列并独立滚动。
 * 左键保持直接粘贴；排序只走右键菜单，不引入拖动排序。
 */
const PinnedPanel: FC = () => {
  useClipboardWindowEditableFocus();

  const { t } = useTranslation("clipboard");
  const settings = useSnapshot(settingsState);
  const [noteTarget, setNoteTarget] = useState<ClipboardItem | null>(null);
  const [orderTarget, setOrderTarget] = useState<ClipboardItem | null>(null);
  const visibleRef = useRef(false);
  const {
    findItemById,
    getItem,
    loadRange,
    loadedInitial,
    loading,
    reload,
    reloadCurrentRange,
    total,
  } = useClipboardItems({
    pinned: true,
    sort: settings.clipboard.content.sort,
  });

  const handleVisibility = (event: { payload: WindowVisibilityPayload }) => {
    const payload = event.payload;
    if (payload.label !== WINDOW_LABEL.PINNED_PANEL) return;

    visibleRef.current = payload.visible;
    if (payload.visible) reload();
  };

  useTauriListen<WindowVisibilityPayload>(
    TAURI_EVENT.WINDOW_VISIBILITY,
    handleVisibility,
  );

  const handleClipboardUpdated = () => {
    if (!visibleRef.current) return;

    reloadCurrentRange();
  };

  useTauriListen(TAURI_EVENT.CLIPBOARD_UPDATED, handleClipboardUpdated);

  const handleMove = async (item: ClipboardItem, position: number) => {
    await movePinnedClipboardItemOrder(item.id, position);
    reloadCurrentRange();
  };

  const handlePositionSubmit = async (position: number) => {
    if (!orderTarget) return;

    await handleMove(orderTarget, position);
    setOrderTarget(null);
  };

  const handleMenuAction = async (payload: ClipboardMenuActionPayload) => {
    const item = findItemById(payload.itemId);
    if (!item) return;

    switch (payload.action) {
      case "paste":
        await pasteClipboardItem(item.id, false);
        return;
      case "pasteAsPlainText":
      case "pasteAsPath":
        await pasteClipboardItem(item.id, true);
        return;
      case "copy":
        await writeToClipboard(item.id, false);
        return;
      case "saveImage":
        await saveClipboardImageToFile(item.id);
        return;
      case "openLink":
        await openClipboardItemLink(item.id, false);
        return;
      case "sendEmail":
        await openClipboardItemLink(item.id, true);
        return;
      case "revealInFinder":
      case "revealInExplorer":
        await revealClipboardItem(item.id);
        return;
      case "toggleFavorite":
        await toggleClipboardItemFavorite(item.id, !item.isFavorite);
        reloadCurrentRange();
        return;
      case "togglePinned":
        await toggleClipboardItemPinned(item.id, false);
        reloadCurrentRange();
        return;
      case "movePinnedFirst":
        await handleMove(item, 1);
        return;
      case "movePinnedLast":
        await handleMove(item, Number.MAX_SAFE_INTEGER);
        return;
      case "movePinnedToPosition":
        setOrderTarget(item);
        return;
      case "moveToGroup":
        if (!payload.groupId) return;
        await updateClipboardItemGroup(item.id, payload.groupId);
        reloadCurrentRange();
        return;
      case "editNote":
        setNoteTarget(item);
        return;
      case "delete": {
        const deleted = await deleteClipboardItem(
          item.id,
          item.isFavorite,
          item.isPinned,
        );
        if (deleted) reloadCurrentRange();
        return;
      }
      case "addToRanking":
      case "moveRankingFirst":
      case "moveRankingLast":
      case "moveRankingToPosition":
      case "removeFromRanking":
        return;
    }
  };

  const handleMenuEvent = (event: { payload: unknown }) => {
    void handleMenuAction(event.payload as ClipboardMenuActionPayload);
  };

  useTauriListen(TAURI_EVENT.CLIPBOARD_MENU_ACTION, handleMenuEvent);

  const handleNoteSaved = (
    _id: string,
    _note: string | null,
    _autoFavorited: boolean,
  ) => {
    reloadCurrentRange();
  };

  const renderItem = (index: number) => {
    const item = getItem(index);
    if (!item) {
      return (
        <div className="min-h-28 rounded-2 border border-ant-border-secondary bg-ant-fill-quaternary" />
      );
    }

    const handleMouseDown = (event: MouseEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;

      void pasteClipboardItem(item.id, false);
    };

    return (
      <ClipboardCard
        availableActions={item.availableActions}
        item={item}
        onMouseDown={handleMouseDown}
      />
    );
  };

  return (
    <div className="flex size-screen flex-col overflow-hidden rounded-3 bg-ant-container">
      <div
        className="flex h-11 shrink-0 items-center justify-between border-ant-border-secondary border-b px-3"
        data-tauri-drag-region
      >
        <div className="flex items-center gap-2 font-medium">
          <i className="i-ph:push-pin-bold size-4" />
          <span>{t("pinnedPanel.title")}</span>
        </div>
        <span className="text-ant-secondary text-xs">{total}</span>
      </div>

      <div className="min-h-0 flex-1">
        {!loadedInitial || loading ? (
          <div className="flex size-full items-center justify-center">
            <Spin />
          </div>
        ) : total === 0 ? (
          <div className="flex size-full items-center justify-center">
            <Empty
              description={t("pinnedPanel.empty")}
              image={Empty.PRESENTED_IMAGE_SIMPLE}
            />
          </div>
        ) : (
          <VirtuosoGrid
            components={{ Item: GridItem, List: GridList }}
            itemContent={renderItem}
            rangeChanged={(range) => {
              loadRange(range.startIndex, range.endIndex);
            }}
            totalCount={total}
          />
        )}
      </div>

      <NoteModal
        item={noteTarget}
        onClose={() => {
          setNoteTarget(null);
        }}
        onSaved={handleNoteSaved}
      />

      <OrderPositionModal
        currentPosition={orderTarget?.pinOrder ?? 1}
        onCancel={() => {
          setOrderTarget(null);
        }}
        onSubmit={handlePositionSubmit}
        open={orderTarget !== null}
        title={t("ranking.moveToPositionTitle")}
      />
    </div>
  );
};

export default PinnedPanel;
