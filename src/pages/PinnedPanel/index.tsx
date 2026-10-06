import { Dropdown, Empty, Spin } from "antd";
import type { MenuProps } from "antd";
import type { FC, MouseEvent as ReactMouseEvent } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Virtuoso } from "react-virtuoso";
import {
  movePinnedClipboardItem,
  pasteClipboardItem,
  toggleClipboardItemPinned,
} from "@/commands";
import { TAURI_EVENT } from "@/constants/events";
import { useClipboardItems } from "@/hooks/useClipboardItems";
import { useClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";
import { useTauriListen } from "@/hooks/useTauriListen";
import type { ClipboardItem } from "@/types/clipboard";
import ClipboardCard from "../Clipboard/components/cards/ClipboardCard";
import OrderPositionModal from "../Clipboard/components/OrderPositionModal";

interface ClipboardUpdatedPayload {
  metadata?: boolean;
}

const PinnedPanel: FC = () => {
  useClipboardWindowEditableFocus();

  const { t } = useTranslation("clipboard");
  const [orderTarget, setOrderTarget] = useState<ClipboardItem | null>(null);
  const { getItem, loadedInitial, loading, loadRange, reload, total } =
    useClipboardItems({
      pinned: true,
      sort: "updatedAtDesc",
    });

  useTauriListen<ClipboardUpdatedPayload>(
    TAURI_EVENT.CLIPBOARD_UPDATED,
    () => {
      reload();
    },
  );

  const rowCount = Math.ceil(total / 2);

  if (loading && !loadedInitial) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-ant-container">
        <Spin />
      </div>
    );
  }

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden rounded-4 bg-ant-container">
      <div
        className="flex h-12 shrink-0 items-center gap-2 border-ant-border-secondary border-b px-3 font-medium"
        data-tauri-drag-region
      >
        <i className="i-ph:push-pin-bold text-ant-primary text-lg" />
        <span>{t("pinnedPanel.title")}</span>
        <span className="ml-auto text-ant-secondary text-xs">{total}</span>
      </div>

      {loadedInitial && total === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <Empty
            description={t("pinnedPanel.empty")}
            image={Empty.PRESENTED_IMAGE_SIMPLE}
          />
        </div>
      ) : (
        <Virtuoso
          className="flex-1"
          itemContent={(rowIndex) => {
            return renderRow(rowIndex);
          }}
          rangeChanged={({ startIndex, endIndex }) => {
            loadRange(startIndex * 2, endIndex * 2 + 1);
          }}
          totalCount={rowCount}
        />
      )}

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
    </div>
  );

  function renderRow(rowIndex: number) {
    const left = getItem(rowIndex * 2);
    const right = getItem(rowIndex * 2 + 1);

    return (
      <div className="grid grid-cols-2 gap-2 px-2 pt-2">
        {left ? renderPinnedCard(left) : <div />}
        {right ? renderPinnedCard(right) : <div />}
      </div>
    );
  }

  function renderPinnedCard(item: ClipboardItem) {
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

    const handleMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;

      pasteClipboardItem(item.id, false);
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

export default PinnedPanel;
