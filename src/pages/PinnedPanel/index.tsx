import { Empty, Spin } from "antd";
import type { FC, MouseEvent as ReactMouseEvent } from "react";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
import { useSnapshot } from "valtio";
import {
  deleteClipboardItem,
  moveClipboardItemPinOrder,
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
import { useClipboardItems } from "@/hooks/useClipboardItems";
import { useTauriListen } from "@/hooks/useTauriListen";
import { settingsState } from "@/stores/settings";
import type { ClipboardAction, ClipboardItem } from "@/types/clipboard";
import ClipboardCard from "@/pages/Clipboard/components/cards/ClipboardCard";
import NoteModal from "@/pages/Clipboard/components/NoteModal";
import OrderPositionModal from "@/pages/Clipboard/components/OrderPositionModal";

interface ClipboardMenuActionPayload {
  action: ClipboardAction;
  groupId?: string;
  itemId: string;
}

const COLUMN_COUNT = 2;

/**
 * 独立置顶面板：只读取 pinned 条目，按 pin_order 两列展示。
 * 左键行为沿用主列表 autoPaste 设置，不引入拖动排序。
 */
const PinnedPanel: FC = () => {
  const { t } = useTranslation("clipboard");
  const settings = useSnapshot(settingsState);
  const autoPaste = settings.clipboard.content.autoPaste;
  const sort = settings.clipboard.content.sort;
  const [noteTarget, setNoteTarget] = useState<ClipboardItem | null>(null);
  const [positionTargetId, setPositionTargetId] = useState<string | null>(null);
  const virtuosoRef = useRef<VirtuosoHandle>(null);

  const {
    findItemById,
    getItem,
    loadRange,
    loadedInitial,
    loading,
    reloadCurrentRange,
    total,
  } = useClipboardItems({
    pinned: true,
    sort,
  });

  const rowCount = Math.ceil(total / COLUMN_COUNT);

  const reloadPanel = () => {
    reloadCurrentRange();
  };

  useTauriListen(TAURI_EVENT.CLIPBOARD_ORDER_UPDATED, reloadPanel);
  useTauriListen(TAURI_EVENT.CLIPBOARD_UPDATED, reloadPanel);

  const handleMovePinOrder = async (id: string, position: number) => {
    await moveClipboardItemPinOrder(id, position);
    reloadCurrentRange();
  };

  const handlePositionConfirm = async (position: number) => {
    const targetId = positionTargetId;
    if (!targetId) return;

    await handleMovePinOrder(targetId, position);
    setPositionTargetId(null);
  };

  const handleMenuAction = async (payload: ClipboardMenuActionPayload) => {
    const target = findItemById(payload.itemId);
    if (!target) return;

    switch (payload.action) {
      case "paste":
        await pasteClipboardItem(target.id, false);
        return;
      case "pasteAsPlainText":
      case "pasteAsPath":
        await pasteClipboardItem(target.id, true);
        return;
      case "copy":
        await writeToClipboard(target.id, false);
        return;
      case "saveImage":
        await saveClipboardImageToFile(target.id);
        return;
      case "openLink":
        await openClipboardItemLink(target.id, false);
        return;
      case "sendEmail":
        await openClipboardItemLink(target.id, true);
        return;
      case "revealInFinder":
      case "revealInExplorer":
        await revealClipboardItem(target.id);
        return;
      case "toggleFavorite":
        await toggleClipboardItemFavorite(target.id, !target.isFavorite);
        reloadCurrentRange();
        return;
      case "togglePinned":
        await toggleClipboardItemPinned(target.id, false);
        reloadCurrentRange();
        return;
      case "moveToGroup":
        if (!payload.groupId) return;

        await updateClipboardItemGroup(target.id, payload.groupId);
        reloadCurrentRange();
        return;
      case "editNote":
        setNoteTarget(target);
        return;
      case "delete": {
        const deleted = await deleteClipboardItem(
          target.id,
          target.isFavorite,
          target.isPinned,
        );
        if (deleted) reloadCurrentRange();
        return;
      }
      case "pinOrderFirst":
        await handleMovePinOrder(target.id, 1);
        return;
      case "pinOrderLast":
        await handleMovePinOrder(target.id, Number.MAX_SAFE_INTEGER);
        return;
      case "pinOrderMoveTo":
        setPositionTargetId(target.id);
        return;
      case "addToManualOrder":
      case "manualOrderFirst":
      case "manualOrderLast":
      case "manualOrderMoveTo":
      case "manualOrderRemove":
        return;
    }
  };

  const handleMenuActionEvent = (event: { payload: unknown }) => {
    void handleMenuAction(event.payload as ClipboardMenuActionPayload);
  };

  useTauriListen(TAURI_EVENT.CLIPBOARD_MENU_ACTION, handleMenuActionEvent);

  if (loading && !loadedInitial) {
    return (
      <div className="flex h-screen items-center justify-center bg-ant-container">
        <Spin />
      </div>
    );
  }

  return (
    <div className="flex h-screen flex-col overflow-hidden rounded-3 bg-ant-container p-2">
      <div className="flex h-9 shrink-0 items-center px-2 font-medium text-ant-text">
        {t("ordering.pinnedTitle")}
      </div>

      {loadedInitial && total === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <Empty
            description={t("ordering.pinnedEmpty")}
            image={Empty.PRESENTED_IMAGE_SIMPLE}
          />
        </div>
      ) : (
        <div className="min-h-0 flex-1">
          <Virtuoso
            itemContent={(rowIndex) => {
              return renderRow(rowIndex);
            }}
            rangeChanged={({ endIndex, startIndex }) => {
              loadRange(
                startIndex * COLUMN_COUNT,
                endIndex * COLUMN_COUNT + (COLUMN_COUNT - 1),
              );
            }}
            ref={virtuosoRef}
            totalCount={rowCount}
          />
        </div>
      )}

      <NoteModal
        item={noteTarget}
        onClose={() => {
          setNoteTarget(null);
        }}
        onSaved={() => {
          setNoteTarget(null);
          reloadCurrentRange();
        }}
      />

      <OrderPositionModal
        onCancel={() => {
          setPositionTargetId(null);
        }}
        onConfirm={handlePositionConfirm}
        open={positionTargetId !== null}
      />
    </div>
  );

  function renderRow(rowIndex: number) {
    const firstIndex = rowIndex * COLUMN_COUNT;
    const indexes = [firstIndex, firstIndex + 1];

    return (
      <div className="grid grid-cols-2 gap-2 px-1 pb-2">
        {indexes.map((index) => {
          if (index >= total) {
            return <div key={index} />;
          }

          const item = getItem(index);
          if (!item) {
            return (
              <div
                className="min-h-24 rounded-2 border border-ant-border-secondary bg-ant-fill-quaternary"
                key={index}
              />
            );
          }

          return renderPinnedCard(item);
        })}
      </div>
    );
  }

  function renderPinnedCard(item: ClipboardItem) {
    const handleMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;

      if (autoPaste === "singleClickPaste") {
        void pasteClipboardItem(item.id, false);
        return;
      }

      if (autoPaste === "singleClickCopy") {
        void writeToClipboard(item.id, false);
      }
    };

    const handleDoubleClick = () => {
      if (autoPaste === "doubleClickPaste") {
        void pasteClipboardItem(item.id, false);
        return;
      }

      if (autoPaste === "doubleClickCopy") {
        void writeToClipboard(item.id, false);
      }
    };

    return (
      <ClipboardCard
        availableActions={item.availableActions}
        item={item}
        key={item.id}
        onDoubleClick={handleDoubleClick}
        onMouseDown={handleMouseDown}
        quickActions={[]}
      />
    );
  }
};

export default PinnedPanel;
