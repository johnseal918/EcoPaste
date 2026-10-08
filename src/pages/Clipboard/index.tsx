import { useEffect } from "react";
import { useClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";
import { log } from "@/utils/log";
import Footer from "./components/Footer";
import Group from "./components/Group";
import Header from "./components/Header";
import List from "./components/List";

const Clipboard = () => {
  useClipboardWindowEditableFocus();

  // 记录主 WebView 是否真实渲染，便于区分原生 HWND 层级和 React 首屏故障。
  useEffect(() => {
    log.info("clipboard main React view mounted");
  }, []);

  return (
    <div
      className="clipboard-main-frame flex size-screen flex-col overflow-hidden bg-ant-container"
      data-tauri-drag-region
    >
      <Header />

      <Group />

      <List />

      <Footer />
    </div>
  );
};

export default Clipboard;
