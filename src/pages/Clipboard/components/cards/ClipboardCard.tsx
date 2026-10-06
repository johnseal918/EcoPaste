import type { DragEvent, FC, MouseEvent, PointerEvent, Ref } from "react";
import { useState } from "react";
import { popupClipboardItemMenu, startDragClipboardItem } from "@/commands";
import KeyHint from "@/components/KeyHint";
import type { ItemActionLabels } from "@/constants/itemActions";
import type { ClipboardAction, ClipboardItem } from "@/types/clipboard";
import type { ItemAction } from "@/types/settings";
import { cn } from "@/utils/cn";
import ClipboardQuickActions from "./ClipboardQuickActions";
import FilesCard from "./FilesCard";
import ImageCard from "./ImageCard";
import NoteContentSwitcher from "./NoteContentSwitcher";
import TextCard from "./TextCard";

interface ClipboardCardProps {
  item: ClipboardItem;
  isSelected?: boolean;
  /**
   * 快捷键提示字符（"1"–"9" / "0"），存在时在 app 图标上叠加 KeyHint；
   * 按下修饰键（macOS ⌘ / Windows Ctrl）+ 该数字键触发快速粘贴。
   */
  hintKey?: string;
  /**
   * 快捷键触发时执行的粘贴操作，由父级列表注入。
   */
  onQuickPaste?: () => void;
  /**
   * MOD 键按下时，URL / Email 文本以链接态展示。
   */
  isLinkActive?: boolean;
  /**
   * 点击 URL / Email 文本时打开外部链接。
   */
  onOpenLink?: () => void;
  onPointerEnter?: (event: PointerEvent<HTMLDivElement>) => void;
  onPointerLeave?: () => void;
  onPointerMove?: (event: PointerEvent<HTMLDivElement>) => void;
  onMouseDown?: (event: MouseEvent<HTMLDivElement>) => void;
  onAuxClick?: (event: MouseEvent<HTMLDivElement>) => void;
  onDoubleClick?: (event: MouseEvent<HTMLDivElement>) => void;
  availableActions?: ClipboardAction[];
  disableContextMenu?: boolean;
  quickActions?: ItemAction[];
  quickActionLabels?: ItemActionLabels;
  onQuickAction?: (action: ItemAction) => Promise<void> | void;
  showOriginalOnHover?: boolean;
  rootRef?: Ref<HTMLDivElement>;
}

/**
 * 按 `kind` 分发到具体卡片组件。卡片不再保留“来源应用 + HTML/文本/链接/图片”
 * 的独立标题行，把纵向空间优先留给真实剪贴板内容；快捷动作仅在 hover 时叠加显示。
 * `isSelected` 为 true 时高亮背景与边框；指针事件由列表注入用于 hover preview。
 */
const ClipboardCard: FC<ClipboardCardProps> = (props) => {
  const {
    item,
    isSelected,
    hintKey,
    onQuickPaste,
    isLinkActive,
    onOpenLink,
    onPointerEnter,
    onPointerLeave,
    onPointerMove,
    onMouseDown,
    onAuxClick,
    onDoubleClick,
    availableActions,
    disableContextMenu = false,
    quickActions = [],
    quickActionLabels,
    onQuickAction,
    showOriginalOnHover = true,
    rootRef,
  } = props;
  const [hovered, setHovered] = useState(false);
  const body = renderBody(item, isLinkActive, onOpenLink);
  const showSensitiveIndicator = item.isSensitive && item.kind === "text";

  const handleDragStart = async (event: DragEvent) => {
    event.preventDefault();

    await startDragClipboardItem(item.id);
  };

  const handleContextMenu = async (event: MouseEvent) => {
    event.preventDefault();
    if (disableContextMenu) return;

    const actions = availableActions ?? item.availableActions ?? [];
    const { isFavorite, isPinned, note } = item;

    if (actions.length === 0) return;

    await popupClipboardItemMenu(
      item.id,
      [...actions],
      item.groupId,
      isFavorite,
      isPinned,
      Boolean(note),
    );
  };

  const handlePointerEnter = (event: PointerEvent<HTMLDivElement>) => {
    setHovered(true);
    onPointerEnter?.(event);
  };

  const handlePointerLeave = () => {
    setHovered(false);
    onPointerLeave?.();
  };

  return (
    <div
      aria-selected={isSelected}
      className={cn(
        "relative flex flex-col gap-1 overflow-hidden rounded-2 border border-ant-border p-2 transition-colors duration-150 ease-out motion-reduce:transition-none",
        {
          "border-ant-primary bg-ant-blue-1": isSelected,
          "border-ant-warning bg-ant-warning-bg":
            item.priorityOrder !== null && !item.isPinned && !isSelected,
        },
      )}
      draggable
      onAuxClick={onAuxClick}
      onContextMenu={handleContextMenu}
      onDoubleClick={onDoubleClick}
      onDragStart={handleDragStart}
      onMouseDown={onMouseDown}
      onPointerEnter={handlePointerEnter}
      onPointerLeave={handlePointerLeave}
      onPointerMove={onPointerMove}
      ref={rootRef}
      role="option"
      tabIndex={-1}
    >
      {hintKey ? (
        <div className="absolute top-2 left-2 z-10">
          <KeyHint hintKey={hintKey} onKeyPress={onQuickPaste}>
            <span className="size-4" />
          </KeyHint>
        </div>
      ) : null}

      {hovered &&
      quickActions.length > 0 &&
      quickActionLabels &&
      onQuickAction ? (
        <div className="absolute top-2 right-2 z-10 rounded-1.5 bg-ant-elevated shadow-sm">
          <ClipboardQuickActions
            item={item}
            labels={quickActionLabels}
            onQuickAction={onQuickAction}
            quickActions={quickActions}
            visible
          />
        </div>
      ) : null}

      <div className={cn({ "pl-6": Boolean(hintKey) })}>
        {item.note ? (
          <NoteContentSwitcher
            note={item.note}
            showOriginal={showOriginalOnHover && hovered}
          >
            {body}
          </NoteContentSwitcher>
        ) : (
          body
        )}
      </div>
      {showSensitiveIndicator ? renderStatusIndicators() : null}
    </div>
  );
};

/**
 * 渲染卡片右下角的状态水印；仅表达状态，不参与交互。
 */
function renderStatusIndicators() {
  return (
    <div className="pointer-events-none absolute right-2 bottom-2 flex items-end gap-1 text-ant-quaternary">
      <i aria-hidden="true" className="i-lucide:key-round size-5" />
    </div>
  );
}

const renderBody = (
  item: ClipboardItem,
  isLinkActive?: boolean,
  onOpenLink?: () => void,
) => {
  if (item.kind === "image") return <ImageCard {...item} />;

  if (item.kind === "files") return <FilesCard {...item} />;

  return (
    <TextCard {...item} isLinkActive={isLinkActive} onOpenLink={onOpenLink} />
  );
};

export default ClipboardCard;
