import { useCallback, useEffect, useMemo, useRef, useState } from "react";

type DappFile = { path: string; size: number };

type DappWorkspaceProps = {
  projectId: string;
  projectPath: string;
  interactive: boolean;
  refreshToken: number;
};

export default function DappWorkspace({
  projectId,
  projectPath,
  interactive,
  refreshToken,
}: DappWorkspaceProps) {
  const [files, setFiles] = useState<DappFile[]>([]);
  const [selectedPath, setSelectedPath] = useState("index.html");
  const [editorValue, setEditorValue] = useState("");
  const [loadedValue, setLoadedValue] = useState("");
  const [previewKey, setPreviewKey] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [panelOpen, setPanelOpen] = useState(interactive);
  const dirtyRef = useRef(false);
  const selectedPathRef = useRef(selectedPath);

  useEffect(() => {
    setPanelOpen(interactive);
  }, [interactive, projectPath]);

  useEffect(() => {
    selectedPathRef.current = selectedPath;
  }, [selectedPath]);

  const dirty = editorValue !== loadedValue;
  dirtyRef.current = dirty;
  const previewSrc = useMemo(
    () => `/api/dapp/preview/${encodeURIComponent(projectId)}?v=${previewKey}`,
    [projectId, previewKey],
  );

  const loadFiles = useCallback(async () => {
    try {
      const res = await fetch(
        `/api/dapp?project_path=${encodeURIComponent(projectPath)}`,
        { cache: "no-store" },
      );
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || `Failed to load dapp (${res.status})`);
      setFiles(json.files ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [projectPath]);

  const loadFile = useCallback(async (path: string) => {
    try {
      const res = await fetch(
        `/api/dapp/file?project_path=${encodeURIComponent(projectPath)}&path=${encodeURIComponent(path)}`,
        { cache: "no-store" },
      );
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || `Failed to read file (${res.status})`);
      setSelectedPath(json.path ?? path);
      setEditorValue(json.content ?? "");
      setLoadedValue(json.content ?? "");
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [projectPath]);

  useEffect(() => {
    void loadFiles();
    void loadFile("index.html");
    setPreviewKey(0);
  }, [loadFiles, loadFile, projectPath]);

  const applyDappUpdate = useCallback(() => {
    setPreviewKey((value) => value + 1);
    void loadFiles();
    if (!dirtyRef.current) {
      void loadFile(selectedPathRef.current);
    }
  }, [loadFiles, loadFile]);

  useEffect(() => {
    if (refreshToken === 0) return;
    applyDappUpdate();
  }, [refreshToken, applyDappUpdate]);

  useEffect(() => {
    let revision = "";
    let cancelled = false;

    const poll = async () => {
      try {
        const res = await fetch(
          `/api/dapp/revision?project_path=${encodeURIComponent(projectPath)}`,
          { cache: "no-store" },
        );
        const json = await res.json();
        if (!res.ok) throw new Error(json.error || `Revision check failed (${res.status})`);
        const next = String(json.revision ?? "");
        if (!cancelled && revision && next !== revision) {
          applyDappUpdate();
        }
        revision = next;
      } catch {
        /* non-critical background poll */
      }
    };

    void poll();
    const timer = window.setInterval(() => void poll(), 1500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [projectPath, applyDappUpdate]);

  const saveFile = async () => {
    setSaving(true);
    setError(null);
    try {
      const res = await fetch("/api/dapp/file", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          project_path: projectPath,
          path: selectedPath,
          content: editorValue,
        }),
      });
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || `Save failed (${res.status})`);
      setLoadedValue(editorValue);
      await loadFiles();
      applyDappUpdate();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const runPreview = () => {
    applyDappUpdate();
  };

  return (
    <div className={`dapp${interactive ? " dapp--interactive" : ""}`}>
      {panelOpen && (
        <aside className="dapp__panel" aria-label="Dapp files">
          <div className="dapp__panel-head">
            <span className="dapp__panel-title">.neura/dapp</span>
            <button type="button" className="dapp__mini-btn" onClick={() => void loadFiles()} title="Refresh files">
              ↻
            </button>
          </div>
          <div className="dapp__files">
            {files.map((file) => (
              <button
                key={file.path}
                type="button"
                className={`dapp__file${file.path === selectedPath ? " is-active" : ""}`}
                onClick={() => void loadFile(file.path)}
                title={file.path}
              >
                {file.path}
              </button>
            ))}
          </div>
          <div className="dapp__editor-wrap">
            <div className="dapp__editor-head">
              <span className="dapp__editor-path">{selectedPath}</span>
              <div className="dapp__editor-actions">
                <button type="button" className="dapp__mini-btn" onClick={runPreview} title="Run preview">
                  Run
                </button>
                <button
                  type="button"
                  className="dapp__mini-btn dapp__mini-btn--primary"
                  onClick={() => void saveFile()}
                  disabled={!dirty || saving}
                >
                  {saving ? "Saving…" : "Save"}
                </button>
              </div>
            </div>
            <textarea
              className="dapp__editor"
              value={editorValue}
              onChange={(e) => setEditorValue(e.target.value)}
              spellCheck={false}
            />
          </div>
          {error && <p className="dapp__error">{error}</p>}
        </aside>
      )}
      <div className="dapp__stage">
        <button
          type="button"
          className="dapp__panel-toggle"
          onClick={() => setPanelOpen((open) => !open)}
          title={panelOpen ? "Hide file panel" : "Show file panel"}
        >
          {panelOpen ? "◧" : "◨"}
        </button>
        <iframe
          key={previewSrc}
          className="dapp__frame"
          title="Project dapp preview"
          src={previewSrc}
          sandbox="allow-scripts allow-forms allow-popups"
        />
      </div>
    </div>
  );
}
