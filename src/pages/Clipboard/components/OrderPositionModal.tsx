import { InputNumber, Modal } from "antd";
import type { FC } from "react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

interface OrderPositionModalProps {
  open: boolean;
  onCancel: () => void;
  onConfirm: (position: number) => Promise<void> | void;
}

/**
 * 精确移动排序位置的小弹窗。只接受 1-based 正整数；
 * 超过当前数量由 Rust 夹到末尾，避免前端维护第二套数量规则。
 */
const OrderPositionModal: FC<OrderPositionModalProps> = (props) => {
  const { open, onCancel, onConfirm } = props;
  const { t } = useTranslation("clipboard");
  const [position, setPosition] = useState<number | null>(1);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;

    setPosition(1);
    setSaving(false);
  }, [open]);

  const handleOk = async () => {
    if (position === null || position < 1) return;

    setSaving(true);
    try {
      await onConfirm(Math.floor(position));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      cancelText={t("ordering.cancel")}
      centered
      okButtonProps={{ disabled: position === null || position < 1 }}
      okText={t("ordering.confirm")}
      onCancel={onCancel}
      onOk={handleOk}
      open={open}
      title={t("ordering.moveToPosition")}
      confirmLoading={saving}
    >
      <InputNumber
        autoFocus
        className="w-full"
        min={1}
        onChange={(value) => {
          setPosition(value);
        }}
        placeholder={t("ordering.positionPlaceholder")}
        precision={0}
        value={position}
      />
    </Modal>
  );
};

export default OrderPositionModal;
