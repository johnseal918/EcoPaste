import { type GetRef, Button, Input, Modal, Spin, Typography } from "antd";
import type { FC } from "react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  copyEditedClipboardText,
  getEditableClipboardText,
  pasteEditedClipboardText,
  saveEditedClipboardText,
} from "@/commands";
import { prepareClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";

interface EditContentModalProps {
  sourceId: string | null;
  onClose: () => void;
}

type TextAreaRef = GetRef<typeof Input.TextArea>;
type EditorAction = "copy" | "save" | "paste";

/**
 * 复用 NoteModal 的交互方式，但始终加载完整正文。
 * 只改临时输出或插入独立记录，不修改原条目的内容/排序/收藏/置顶。
 */
const EditContentModal: FC<EditContentModalProps> = ({ sourceId, onClose }) => {
  const { t } = useTranslation("clipboard");
  const textAreaRef = useRef<TextAreaRef>(null);
  const [original, setOriginal] = useState("");
  const [value, setValue] = useState("");
  const [fromRich, setFromRich] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [busy, setBusy] = useState<EditorAction | null>(null);

  useEffect(() => {
    if (!sourceId) return;
    let current = true;
    setOriginal("");
    setValue("");
    setFromRich(false);
    setLoadFailed(false);
    setLoading(true);
    setBusy(null);

    const load = async () => {
      try {
        // Windows 剪贴板浮窗默认不可聚焦；先复用已有输入焦点管理。
        await prepareClipboardWindowEditableFocus();
        const result = await getEditableClipboardText(sourceId);
        if (!current) return;
        setOriginal(result.text);
        setValue(result.text);
        setFromRich(result.plainTextFromRich);
      } catch {
        // 命令层已经输出失败提示；弹窗保留可见的错误状态，不展示摘要当作正文。
        if (current) setLoadFailed(true);
      } finally {
        if (current) setLoading(false);
      }
    };
    void load();
    return () => {
      current = false;
    };
  }, [sourceId]);

  useEffect(() => {
    if (!sourceId || loading || loadFailed) return;
    requestAnimationFrame(() => {
      textAreaRef.current?.focus({ cursor: "end" });
    });
  }, [sourceId, loading, loadFailed]);

  const handleAction = async (action: EditorAction) => {
    if (!sourceId || busy || loading || loadFailed || !value.trim()) return;
    if (action === "save" && value === original) return;
    setBusy(action);
    try {
      if (action === "copy") {
        await copyEditedClipboardText(sourceId, value);
      } else if (action === "save") {
        await saveEditedClipboardText(sourceId, value);
        onClose();
      } else {
        await pasteEditedClipboardText(sourceId, value);
        onClose();
      }
    } catch {
      // 错误已由 commands 统一提示；保留未保存的编辑内容以便重试。
    } finally {
      setBusy(null);
    }
  };

  const ready = !loading && !loadFailed && busy === null;
  const canOutput = ready && value.trim().length > 0;

  return (
    <Modal
      afterOpenChange={(open) => {
        if (open && !loading && !loadFailed) {
          textAreaRef.current?.focus({ cursor: "end" });
        }
      }}
      closable={busy === null}
      destroyOnHidden
      footer={
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap justify-start gap-2">
            <Button disabled={!ready || value === original} onClick={() => setValue(original)} size="small">
              {t("contentEditor.reset")}
            </Button>
            <Button
              disabled={!canOutput}
              loading={busy === "copy"}
              onClick={() => void handleAction("copy")}
              size="small"
            >
              {t("contentEditor.copy")}
            </Button>
          </div>
          <div className="flex flex-wrap justify-end gap-2">
            <Button disabled={busy !== null} onClick={onClose}>
              {t("contentEditor.cancel")}
            </Button>
            <Button
              disabled={!canOutput || value === original}
              loading={busy === "save"}
              onClick={() => void handleAction("save")}
            >
              {t("contentEditor.saveNew")}
            </Button>
            <Button
              disabled={!canOutput}
              loading={busy === "paste"}
              onClick={() => void handleAction("paste")}
              type="primary"
            >
              {t("contentEditor.paste")}
            </Button>
          </div>
        </div>
      }
      maskClosable={false}
      onCancel={() => {
        if (!busy) onClose();
      }}
      open={sourceId !== null}
      title={t("contentEditor.title")}
      width="calc(100vw - 16px)"
    >
      {loading ? (
        <div className="flex justify-center py-6"><Spin /></div>
      ) : loadFailed ? (
        <Typography.Text type="danger">{t("contentEditor.loadFailed")}</Typography.Text>
      ) : (
        <div className="flex flex-col gap-2">
          {fromRich ? (
            <Typography.Text type="secondary">{t("contentEditor.richNotice")}</Typography.Text>
          ) : null}
          <Input.TextArea
            autoSize={{ minRows: 5, maxRows: 9 }}
            disabled={busy !== null}
            onChange={(event) => setValue(event.target.value)}
            onKeyDown={(event) => event.stopPropagation()}
            ref={textAreaRef}
            style={{ maxHeight: "35vh" }}
            value={value}
          />
          <Typography.Text type="secondary">
            {t("contentEditor.preserved")}
          </Typography.Text>
          {value.trim().length === 0 ? (
            <Typography.Text type="warning">{t("contentEditor.empty")}</Typography.Text>
          ) : null}
        </div>
      )}
    </Modal>
  );
};

export default EditContentModal;
