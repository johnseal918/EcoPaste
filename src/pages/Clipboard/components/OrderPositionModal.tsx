import { InputNumber, Modal } from "antd";
import type { FC } from "react";
import { useEffect, useState } from "react";

interface OrderPositionModalProps {
  currentPosition: number;
  open: boolean;
  title: string;
  onCancel: () => void;
  onSubmit: (position: number) => Promise<void> | void;
}

/**
 * 输入 1-based 排序位置；后端负责把超出当前数量的值收敛到末尾。
 */
const OrderPositionModal: FC<OrderPositionModalProps> = (props) => {
  const { currentPosition, onCancel, onSubmit, open, title } = props;
  const [position, setPosition] = useState(currentPosition);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (!open) return;

    setPosition(currentPosition);
  }, [currentPosition, open]);

  const handleOk = async () => {
    if (!Number.isInteger(position) || position < 1) return;

    setSubmitting(true);
    try {
      await onSubmit(position);
    } finally {
      setSubmitting(false);
    }
  };

  const handleChange = (value: number | null) => {
    if (value === null) return;

    setPosition(Math.max(1, Math.trunc(value)));
  };

  return (
    <Modal
      centered
      confirmLoading={submitting}
      destroyOnHidden
      okButtonProps={{ disabled: position < 1 }}
      onCancel={onCancel}
      onOk={handleOk}
      open={open}
      title={title}
    >
      <InputNumber
        autoFocus
        className="w-full"
        min={1}
        onChange={handleChange}
        precision={0}
        value={position}
      />
    </Modal>
  );
};

export default OrderPositionModal;
