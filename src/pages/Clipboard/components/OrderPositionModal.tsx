import { InputNumber, Modal } from "antd";
import type { FC } from "react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

interface OrderPositionModalProps {
  currentPosition?: number | null;
  open: boolean;
  onCancel: () => void;
  onConfirm: (position: number) => Promise<void> | void;
}

/**
 * Manual-order position editor shared by normal history and pinned items.
 */
const OrderPositionModal: FC<OrderPositionModalProps> = (props) => {
  const { currentPosition, open, onCancel, onConfirm } = props;
  const { t } = useTranslation("clipboard");
  const [position, setPosition] = useState<number>(currentPosition ?? 1);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;

    setPosition(Math.max(1, currentPosition ?? 1));
  }, [currentPosition, open]);

  const handleConfirm = async () => {
    setSaving(true);
    try {
      await onConfirm(Math.max(1, position));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      centered
      confirmLoading={saving}
      onCancel={onCancel}
      onOk={handleConfirm}
      open={open}
      title={t("ordering.positionTitle")}
    >
      <div className="flex items-center gap-3 py-2">
        <span className="shrink-0 text-ant-secondary">
          {t("ordering.positionLabel")}
        </span>
        <InputNumber
          autoFocus
          className="w-full"
          min={1}
          onChange={(value) => {
            if (typeof value === "number") setPosition(value);
          }}
          onPressEnter={handleConfirm}
          placeholder={t("ordering.positionPlaceholder")}
          precision={0}
          value={position}
        />
      </div>
    </Modal>
  );
};

export default OrderPositionModal;
