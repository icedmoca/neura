import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type ChangeEvent,
} from "react";
import DappHistoryPanel from "./DappHistoryPanel";
import DappLibraryPanel from "./DappLibraryPanel";
import { useDapp } from "./DappProvider";

type SidebarTab = "files" | "library" | "history";

type ConnectorStyle = {
  left: number;
  top: number;
  height: number;
};

type EditorViewport = {
  scrollTop: number;
  selectionStart: number;
  selectionEnd: number;
};

export default function DappSidebar() {
  const {
    files,
    selectedPath,
    editorValue,
    setEditorValue,
    setEditorFocused,
    error,
    generating,
    libraryHit,
    loadFiles,
    loadFile,
  } = useDapp();

  const stackRef = useRef<HTMLDivElement>(null);
  const filesRef = useRef<HTMLDivElement>(null);
  const activeRef = useRef<HTMLButtonElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const userEditRef = useRef(false);
  const viewportRef = useRef<EditorViewport | null>(null);
  const [connector, setConnector] = useState<ConnectorStyle | null>(null);
  const [tab, setTab] = useState<SidebarTab>("files");

  const captureViewport = useCallback(() => {
    const editor = editorRef.current;
    if (!editor) return;
    viewportRef.current = {
      scrollTop: editor.scrollTop,
      selectionStart: editor.selectionStart,
      selectionEnd: editor.selectionEnd,
    };
  }, []);

  const updateConnector = useCallback(() => {
    const stack = stackRef.current;
    const active = activeRef.current;
    if (!stack || !active) {
      setConnector(null);
      return;
    }

    const stackRect = stack.getBoundingClientRect();
    const activeRect = active.getBoundingClientRect();
    const top = activeRect.bottom - stackRect.top;
    const height = stackRect.height - top;

    if (height <= 0) {
      setConnector(null);
      return;
    }

    setConnector({
      left: activeRect.left - stackRect.left,
      top,
      height,
    });
  }, []);

  useLayoutEffect(() => {
    updateConnector();

    const filesEl = filesRef.current;
    const stackEl = stackRef.current;
    window.addEventListener("resize", updateConnector);
    filesEl?.addEventListener("scroll", updateConnector, { passive: true });

    const observer = new ResizeObserver(updateConnector);
    if (stackEl) observer.observe(stackEl);
    if (filesEl) observer.observe(filesEl);
    if (activeRef.current) observer.observe(activeRef.current);

    return () => {
      window.removeEventListener("resize", updateConnector);
      filesEl?.removeEventListener("scroll", updateConnector);
      observer.disconnect();
    };
  }, [selectedPath, files, updateConnector]);

  useLayoutEffect(() => {
    viewportRef.current = null;
  }, [selectedPath]);

  useLayoutEffect(() => {
    const editor = editorRef.current;
    if (!editor) return;

    if (userEditRef.current) {
      userEditRef.current = false;
      return;
    }

    const saved = viewportRef.current;
    if (!saved) return;

    editor.scrollTop = saved.scrollTop;
    editor.setSelectionRange(saved.selectionStart, saved.selectionEnd);
  }, [editorValue, selectedPath]);

  const handleEditorChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    userEditRef.current = true;
    setEditorValue(event.target.value);
  };

  return (
    <div className="dapp-sidebar">
      <div className="dapp-sidebar__head">
        <span className="dapp-sidebar__label">
          .neura/dapp
          {generating && <span className="dapp-sidebar__sync">syncing</span>}
          {libraryHit && <span className="dapp-sidebar__instant">instant · {libraryHit}</span>}
        </span>
        <button type="button" className="dapp-sidebar__mini-btn" onClick={() => void loadFiles()} title="Refresh files">
          ↻
        </button>
      </div>
      <div className="dapp-sidebar__tabs">
        <button type="button" className={`dapp-sidebar__tab${tab === "files" ? " is-active" : ""}`} onClick={() => setTab("files")}>Files</button>
        <button type="button" className={`dapp-sidebar__tab${tab === "library" ? " is-active" : ""}`} onClick={() => setTab("library")}>Library</button>
        <button type="button" className={`dapp-sidebar__tab${tab === "history" ? " is-active" : ""}`} onClick={() => setTab("history")}>History</button>
      </div>
      {tab === "library" && <DappLibraryPanel />}
      {tab === "history" && <DappHistoryPanel />}
      {tab === "files" && (
      <div className="dapp-sidebar__stack" ref={stackRef}>
        {connector && (
          <span
            className="dapp-sidebar__connector"
            style={{
              left: `${connector.left}px`,
              top: `${connector.top}px`,
              height: `${connector.height}px`,
            }}
            aria-hidden
          />
        )}
        <div className="dapp-sidebar__files" ref={filesRef}>
          {files.map((file) => (
            <button
              key={file.path}
              ref={file.path === selectedPath ? activeRef : undefined}
              type="button"
              className={`dapp-sidebar__file${file.path === selectedPath ? " is-active" : ""}`}
              onClick={() => void loadFile(file.path)}
              title={file.path}
            >
              {file.path}
            </button>
          ))}
        </div>
        <div className="dapp-sidebar__editor-wrap">
          <textarea
            ref={editorRef}
            className="dapp-sidebar__editor"
            value={editorValue}
            onChange={handleEditorChange}
            onFocus={() => setEditorFocused(true)}
            onBlur={() => {
              captureViewport();
              setEditorFocused(false);
            }}
            onScroll={captureViewport}
            onSelect={captureViewport}
            spellCheck={false}
          />
        </div>
      </div>
      )}
      {error && <p className="dapp-sidebar__error">{error}</p>}
    </div>
  );
}
