import {
  defineConfig,
  presetIcons,
  presetWind4,
  transformerDirectives,
  transformerVariantGroup,
} from "unocss";
import { presetAntdColors } from "./src/unocss/presetAntdColors";

export default defineConfig({
  safelist: [
    "i-ph:push-pin-bold",
    "i-lets-icons:star",
    "i-lets-icons:file-dock",
    "i-lets-icons:img-box",
    "i-lets-icons:folder-file-alt",
  ],
  presets: [presetWind4(), presetAntdColors(), presetIcons()],
  transformers: [
    transformerVariantGroup(),
    transformerDirectives({
      applyVariable: ["--uno"],
    }),
  ],
});
