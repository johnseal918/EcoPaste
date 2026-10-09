import { Component, Suspense, type ErrorInfo, type ReactNode } from "react";
import ReactDOM from "react-dom/client";
import { log } from "@/utils/log";
import App from "./App";
import "./i18n";

import "overlayscrollbars/overlayscrollbars.css";
import "virtual:uno.css";
import "./styles/global.scss";

interface BootBoundaryProps {
  children: ReactNode;
}

interface BootBoundaryState {
  error: Error | null;
}

/**
 * Preserve a visible main window when the React tree fails before Clipboard mounts.
 * This is deliberately outside App's ConfigProvider so boot errors are also caught.
 */
class BootBoundary extends Component<BootBoundaryProps, BootBoundaryState> {
  state: BootBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): BootBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    document.getElementById("ecopaste-main-boot")?.remove();
    log.error("EcoPaste main React startup render error", {
      message: error.message,
      name: error.name,
      componentStack: info.componentStack,
      stack: error.stack,
    });
  }

  render() {
    const { error } = this.state;
    if (error) {
      return (
        <div
          role="alert"
          style={{
            alignItems: "center",
            background: "#f6f7fa",
            boxSizing: "border-box",
            color: "#263546",
            display: "flex",
            flexDirection: "column",
            fontFamily: "system-ui, Microsoft YaHei, sans-serif",
            fontSize: 14,
            height: "100vh",
            justifyContent: "center",
            overflowWrap: "anywhere",
            padding: 20,
            textAlign: "center",
          }}
        >
          <strong style={{ fontSize: 16 }}>EcoPaste 主界面渲染失败</strong>
          <p style={{ maxWidth: "100%" }}>{error.message}</p>
          <button onClick={() => window.location.reload()} type="button">
            重新加载主界面
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <BootBoundary>
    <Suspense fallback={null}>
      <App />
    </Suspense>
  </BootBoundary>,
);
