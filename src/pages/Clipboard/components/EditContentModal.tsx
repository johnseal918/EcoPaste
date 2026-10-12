import {
  Button,
  Dropdown,
  type GetRef,
  Input,
  type MenuProps,
  Modal,
  Spin,
  Typography,
} from "antd";
import type { FC } from "react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  copyEditedClipboardText,
  getEditableClipboardText,
  pasteEditedClipboardText,
  saveEditedClipboardText,
  updateEditedClipboardText,
} from "@/commands";
import { prepareClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";

interface EditContentModalProps {
  sourceId: string | null;
  onClose: () => void;
}

type TextAreaRef = GetRef<typeof Input.TextArea>;
type EditorAction = "save" | "savePaste" | "copy" | "saveNew" | "pasteOnly";

/**
 * 主操作是保存当前历史、保存并粘贴和取消。
 * 原有单次粘贴、复制、另存新记录和恢复原文仍可在“更多操作”中使用。
 */
const EditContentModal: FC<EditContentModalProps> = ({ sourceId, onClose }) => {
  const { t } = useTranslation("clipboard");
  const textAreaRef = useRef<TextAreaRef>(null);
  const [original, setOriginal] = useState("");
  const [sourceHash, setSourceHash] = useState("");
  const [value, setValue] = useState("");
  const [fromRich, setFromRich] = useState(false);
  const [loading, setLoading] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [busy, setBusy] = useState<EditorAction | null>(null);

  useEffect(() => {
    if (!sourceId) return;
    let current = true;
    setOriginal("");
    setSourceHash("");
    setValue("");
    setFromRich(false);
    setLoadFailed(false);
    setLoading(true);
    setBusy(null);
    const load = async () => {
      try {
        await prepareClipboardWindowEditableFocus();
        const result = await getEditableClipboardText(sourceId);
        if (!current) return;
        setOriginal(result.text);
        setSourceHash(result.contentHash);
        setValue(result.text);
        setFromRich(result.plainTextFromRich);
      } catch {
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

  const ready =
    !loading && !loadFailed && busy === null && sourceHash.length > 0;
  const canOutput = ready && value.trim().length > 0;
  const changed = value !== original;

  const handleAction = async (action: EditorAction) => {
    if (
      !sourceId ||
      !sourceHash ||
      busy ||
      loading ||
      loadFailed ||
      !value.trim()
    )
      return;
    setBusy(action);
    try {
      if (action === "save" || action === "savePaste") {
        if (changed) {
          await updateEditedClipboardText(
            sourceId,
            sourceHash,
            value,
            action === "savePaste",
          );
        } else if (action === "savePaste") {
          await pasteEditedClipboardText(sourceId, value);
        }
        onClose();
      } else if (action === "copy") {
        await copyEditedClipboardText(sourceId, value);
      } else if (action === "saveNew") {
        await saveEditedClipboardText(sourceId, value);
        onClose();
      } else {
        await pasteEditedClipboardText(sourceId, value);
        onClose();
      }
    } catch {
      // 已由 commands 统一展示失败原因；保留编辑文本方便再次操作。
    } finally {
      setBusy(null);
    }
  };

  const moreMenu: MenuProps = {
    items: [
      { disabled: !canOutput, key: "copy", label: t("contentEditor.copy") },
      {
        disabled: !canOutput,
        key: "pasteOnly",
        label: t("contentEditor.pasteOnce"),
      },
      {
        disabled: !canOutput || !changed,
        key: "saveNew",
        label: t("contentEditor.saveNew"),
      },
      { type: "divider" },
      {
        disabled: !ready || !changed,
        key: "reset",
        label: t("contentEditor.reset"),
      },
    ],
    onClick: ({ key }) => {
      if (key === "reset") {
        setValue(original);
      } else if (key === "copy" || key === "pasteOnly" || key === "saveNew") {
        void handleAction(key);
      }
    },
  };

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
        <div className="flex flex-wrap items-center justify-between gap-2">
          <Dropdown disabled={!ready} menu={moreMenu} trigger={["click"]}>
            <Button size="small" type="text">
              {t("contentEditor.moreActions")}
            </Button>
          </Dropdown>
          <div className="flex flex-wrap justify-end gap-2">
            <Button disabled={busy !== null} onClick={onClose}>
              {t("contentEditor.cancel")}
            </Button>
            <Button
              disabled={!canOutput}
              loading={busy === "savePaste"}
              onClick={() => void handleAction("savePaste")}
            >
              {t("contentEditor.saveAndPaste")}
            </Button>
            <Button
              disabled={!canOutput}
              loading={busy === "save"}
              onClick={() => void handleAction("save")}
              type="primary"
            >
              {t("contentEditor.save")}
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
        <div className="flex justify-center py-6">
          <Spin />
        </div>
      ) : loadFailed ? (
        <Typography.Text type="danger">
          {t("contentEditor.loadFailed")}
        </Typography.Text>
      ) : (
        <div className="flex flex-col gap-2">
          {fromRich ? (
            <Typography.Text type="secondary">
              {t("contentEditor.richNotice")}
            </Typography.Text>
          ) : null}
          <Input.TextArea
            autoSize={{ maxRows: 9, minRows: 5 }}
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
            <Typography.Text type="warning">
              {t("contentEditor.empty")}
            </Typography.Text>
          ) : null}
        </div>
      )}
    </Modal>
  );
};

export default EditContentModal;
