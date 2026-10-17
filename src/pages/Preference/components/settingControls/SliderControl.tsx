import { Slider } from "antd";
import type { FC } from "react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { PreferenceSetting } from "../../types/preferences";
import type { ControlProps } from "./types";

interface SliderControlProps extends ControlProps {
  setting: PreferenceSetting;
  value: number;
}

/**
 * Slider updates the displayed value during drag, then persists once on release.
 * The existing settings store broadcasts the saved value to every WebView.
 */
const SliderControl: FC<SliderControlProps> = (props) => {
  const { disabled, onChange, setting, value } = props;
  const { t } = useTranslation("preferences");
  const [draft, setDraft] = useState(value);
  const control = setting.control.type === "slider" ? setting.control : null;

  useEffect(() => {
    setDraft(value);
  }, [value]);

  if (!control) return null;

  const midpoint =
    Math.round((control.min + control.max) / (2 * control.step)) * control.step;
  const marks = {
    [control.min]: `${control.min}%`,
    [midpoint]: `${midpoint}%`,
    [control.max]: `${control.max}%`,
  };

  const handleComplete = async (next: number) => {
    if (next === value) return;
    await onChange(setting, next);
  };

  return (
    <div className="flex items-start gap-3">
      <Slider
        aria-label={t(`schema.settings.${setting.id}.title`)}
        className="w-44"
        disabled={disabled}
        marks={marks}
        max={control.max}
        min={control.min}
        onChange={setDraft}
        onChangeComplete={(next) => {
          void handleComplete(next);
        }}
        step={control.step}
        value={draft}
      />
      <span className="mt-1 w-11 text-right font-medium text-ant-text text-sm tabular-nums">
        {draft}%
      </span>
    </div>
  );
};

export default SliderControl;
