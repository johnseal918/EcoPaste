import { useEffect } from "react";
import { useClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";
import Footer from "./components/Footer";
import Group from "./components/Group";
import Header from "./components/Header";
import List from "./components/List";

const Clipboard = () => {
  useClipboardWindowEditableFocus();

  // Only remove the independent bootstrap status once this real main route commits.
  useEffect(() => {
    document.getElementById("ecopaste-main-boot")?.remove();
  }, []);

  return (
    <div
      className="flex size-screen flex-col overflow-hidden bg-ant-container"
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
