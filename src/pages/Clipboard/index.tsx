import { useEffect, useState } from "react";
import { TAURI_EVENT } from "@/constants/events";
import { useClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";
import { useTauriListen } from "@/hooks/useTauriListen";
import EditContentModal from "./components/EditContentModal";
import Footer from "./components/Footer";
import Group from "./components/Group";
import Header from "./components/Header";
import List from "./components/List";

const Clipboard = () => {
  useClipboardWindowEditableFocus();
  const [editSourceId, setEditSourceId] = useState<string | null>(null);
  useTauriListen<{ itemId: string }>(
    TAURI_EVENT.CLIPBOARD_EDIT_REQUEST,
    (event) => {
      setEditSourceId(event.payload.itemId);
    },
  );

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

      <List onEditContent={setEditSourceId} />

      <Footer />
      <EditContentModal
        onClose={() => setEditSourceId(null)}
        sourceId={editSourceId}
      />
    </div>
  );
};

export default Clipboard;
