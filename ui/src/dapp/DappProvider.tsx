import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

type DappFile = { path: string; size: number };

type LibraryTemplate = {
  id: string;
  title?: string;
  slug?: string;
  category?: string;
  useCount?: number;
  pinned?: boolean;
};

type DappSnapshot = {
  turn: number;
  createdAt?: number;
};

type DappDiffResult = {
  summary: string;
  changedFiles: string[];
  files: Record<string, { unified?: string }>;
};

type DappContextValue = {
  files: DappFile[];
  selectedPath: string;
  editorValue: string;
  setEditorValue: (value: string) => void;
  setEditorFocused: (focused: boolean) => void;
  dirty: boolean;
  saving: boolean;
  error: string | null;
  generating: boolean;
  libraryHit: string | null;
  previewSrc: string;
  libraryTemplates: LibraryTemplate[];
  libraryLoading: boolean;
  snapshots: DappSnapshot[];
  lastDiff: DappDiffResult | null;
  diffLoading: boolean;
  loadFiles: () => Promise<void>;
  loadFile: (path: string) => Promise<void>;
  saveFile: () => Promise<void>;
  runPreview: () => void;
  loadLibrary: () => Promise<void>;
  applyLibraryTemplate: (templateId: string) => Promise<void>;
  pinLibraryTemplate: (templateId: string) => Promise<void>;
  unpinLibraryTemplate: (templateId: string) => Promise<void>;
  deleteLibraryTemplate: (templateId: string) => Promise<void>;
  loadHistory: () => Promise<void>;
  undoSnapshot: (turn: number) => Promise<void>;
  showDiff: (turn: number) => Promise<void>;
};

const DappContext = createContext<DappContextValue | null>(null);

export function useDapp() {
  const value = useContext(DappContext);
  if (!value) throw new Error("useDapp must be used within DappProvider");
  return value;
}

type DappProviderProps = {
  projectId: string;
  projectPath: string;
  sessionId: string | null;
  refreshToken: number;
  generating: boolean;
  libraryHit: string | null;
  interactive: boolean;
  children: ReactNode;
};

export function DappProvider({
  projectId,
  projectPath,
  sessionId,
  refreshToken,
  generating,
  libraryHit,
  interactive,
  children,
}: DappProviderProps) {
  const [files, setFiles] = useState<DappFile[]>([]);
  const [selectedPath, setSelectedPath] = useState("index.html");
  const [editorValue, setEditorValue] = useState("");
  const [loadedValue, setLoadedValue] = useState("");
  const [previewKey, setPreviewKey] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [libraryTemplates, setLibraryTemplates] = useState<LibraryTemplate[]>([]);
  const [libraryLoading, setLibraryLoading] = useState(false);
  const [snapshots, setSnapshots] = useState<DappSnapshot[]>([]);
  const [lastDiff, setLastDiff] = useState<DappDiffResult | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const dirtyRef = useRef(false);
  const selectedPathRef = useRef(selectedPath);
  const editorValueRef = useRef(editorValue);
  const loadedValueRef = useRef(loadedValue);
  const saveInFlightRef = useRef(false);
  const queuedSaveRef = useRef<{ path: string; content: string } | null>(null);
  const previewReloadTimerRef = useRef<number | null>(null);
  const editorFocusedRef = useRef(false);
  const sessionIdRef = useRef(sessionId);

  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  const activateSession = useCallback(async (sid: string) => {
    const res = await fetch("/api/dapp/activate", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        project_path: projectPath,
        session_id: sid,
      }),
    });
    const json = await res.json();
    if (!res.ok) throw new Error(json.error || `Failed to activate dapp (${res.status})`);
  }, [projectPath]);

  const dirty = editorValue !== loadedValue;
  dirtyRef.current = dirty;
  editorValueRef.current = editorValue;
  loadedValueRef.current = loadedValue;

  useEffect(() => {
    selectedPathRef.current = selectedPath;
  }, [selectedPath]);

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
      const nextFiles: DappFile[] = json.files ?? [];
      setFiles((prev) => {
        if (
          prev.length === nextFiles.length &&
          prev.every((file, index) => {
            const other = nextFiles[index];
            return file.path === other.path && file.size === other.size;
          })
        ) {
          return prev;
        }
        return nextFiles;
      });
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [projectPath]);

  const loadFile = useCallback(async (path: string) => {
    const previousPath = selectedPathRef.current;
    const previousContent = editorValueRef.current;
    if (previousPath !== path && previousContent !== loadedValueRef.current) {
      try {
        const res = await fetch("/api/dapp/file", {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            project_path: projectPath,
            session_id: sessionIdRef.current ?? undefined,
            path: previousPath,
            content: previousContent,
          }),
        });
        const json = await res.json();
        if (!res.ok) throw new Error(json.error || `Save failed (${res.status})`);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }

    if (path === selectedPathRef.current && dirtyRef.current) {
      return;
    }

    try {
      const res = await fetch(
        `/api/dapp/file?project_path=${encodeURIComponent(projectPath)}&path=${encodeURIComponent(path)}`,
        { cache: "no-store" },
      );
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || `Failed to read file (${res.status})`);
      const content = json.content ?? "";
      const resolvedPath = json.path ?? path;
      if (path === selectedPathRef.current && content === editorValueRef.current) {
        setLoadedValue(content);
        setError(null);
        return;
      }
      setSelectedPath(resolvedPath);
      setEditorValue(content);
      setLoadedValue(content);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [projectPath]);

  const reloadPreview = useCallback(() => {
    setPreviewKey((value) => value + 1);
  }, []);

  const schedulePreviewReload = useCallback(() => {
    if (previewReloadTimerRef.current !== null) {
      window.clearTimeout(previewReloadTimerRef.current);
    }
    previewReloadTimerRef.current = window.setTimeout(() => {
      previewReloadTimerRef.current = null;
      reloadPreview();
    }, 120);
  }, [reloadPreview]);

  useEffect(() => () => {
    if (previewReloadTimerRef.current !== null) {
      window.clearTimeout(previewReloadTimerRef.current);
    }
  }, []);

  const loadLibrary = useCallback(async () => {
    setLibraryLoading(true);
    try {
      const res = await fetch("/api/dapp/library", { cache: "no-store" });
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || "Failed to load library");
      setLibraryTemplates(json.templates ?? []);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLibraryLoading(false);
    }
  }, []);

  const loadHistory = useCallback(async () => {
    const sid = sessionIdRef.current;
    if (!sid) {
      setSnapshots([]);
      return;
    }
    try {
      const res = await fetch(
        `/api/dapp/history?project_path=${encodeURIComponent(projectPath)}&session_id=${encodeURIComponent(sid)}`,
        { cache: "no-store" },
      );
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || "Failed to load history");
      setSnapshots(json.snapshots ?? []);
    } catch {
      /* non-critical */
    }
  }, [projectPath]);

  const applyDappUpdate = useCallback(() => {
    reloadPreview();
    void loadFiles();
    void loadHistory();
    void loadLibrary();
    if (!dirtyRef.current && !editorFocusedRef.current) {
      void loadFile(selectedPathRef.current);
    }
  }, [loadFiles, loadFile, loadHistory, loadLibrary, reloadPreview]);

  const applyLibraryTemplate = useCallback(async (templateId: string) => {
    const sid = sessionIdRef.current;
    if (!sid) {
      setError("Open a chat to apply a saved dapp");
      return;
    }
    const res = await fetch("/api/dapp/library/apply", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        project_path: projectPath,
        session_id: sid,
        template_id: templateId,
      }),
    });
    const json = await res.json();
    if (!res.ok) throw new Error(json.error || "Apply failed");
    applyDappUpdate();
  }, [applyDappUpdate, projectPath]);

  const pinLibraryTemplate = useCallback(async (templateId: string) => {
    await fetch("/api/dapp/library/pin", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ template_id: templateId }),
    });
    void loadLibrary();
  }, [loadLibrary]);

  const unpinLibraryTemplate = useCallback(async (templateId: string) => {
    await fetch("/api/dapp/library/unpin", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ template_id: templateId }),
    });
    void loadLibrary();
  }, [loadLibrary]);

  const deleteLibraryTemplate = useCallback(async (templateId: string) => {
    await fetch(`/api/dapp/library/${encodeURIComponent(templateId)}`, { method: "DELETE" });
    void loadLibrary();
  }, [loadLibrary]);

  const undoSnapshot = useCallback(async (turn: number) => {
    const sid = sessionIdRef.current;
    if (!sid) return;
    const res = await fetch("/api/dapp/undo", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        project_path: projectPath,
        session_id: sid,
        turn,
      }),
    });
    const json = await res.json();
    if (!res.ok) throw new Error(json.error || "Undo failed");
    applyDappUpdate();
  }, [applyDappUpdate, projectPath]);

  const showDiff = useCallback(async (turn: number) => {
    const sid = sessionIdRef.current;
    if (!sid) return;
    setDiffLoading(true);
    try {
      const res = await fetch(
        `/api/dapp/diff?project_path=${encodeURIComponent(projectPath)}&session_id=${encodeURIComponent(sid)}&turn=${turn}`,
        { cache: "no-store" },
      );
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || "Diff failed");
      setLastDiff({
        summary: json.summary ?? "",
        changedFiles: json.changedFiles ?? [],
        files: json.files ?? {},
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDiffLoading(false);
    }
  }, [projectPath]);

  const setEditorFocused = useCallback((focused: boolean) => {
    editorFocusedRef.current = focused;
  }, []);

  useEffect(() => {
    let cancelled = false;
    const boot = async () => {
      try {
        if (sessionId) {
          await activateSession(sessionId);
        }
        if (cancelled) return;
        await loadFiles();
        await loadFile("index.html");
        await loadLibrary();
        await loadHistory();
        setPreviewKey((value) => value + 1);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      }
    };
    void boot();
    return () => {
      cancelled = true;
    };
  }, [activateSession, loadFiles, loadFile, loadLibrary, loadHistory, projectPath, sessionId]);

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
    const timer = window.setInterval(() => void poll(), 800);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [projectPath, applyDappUpdate]);

  const persistEditor = useCallback(async (path: string, content: string) => {
    setSaving(true);
    setError(null);
    try {
      const res = await fetch("/api/dapp/file", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          project_path: projectPath,
          session_id: sessionIdRef.current ?? undefined,
          path,
          content,
        }),
      });
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || `Save failed (${res.status})`);
      if (path === selectedPathRef.current && content === editorValueRef.current) {
        setLoadedValue(content);
      }
      schedulePreviewReload();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }, [projectPath, schedulePreviewReload]);

  const drainSaveQueue = useCallback(async () => {
    if (saveInFlightRef.current) return;
    const next = queuedSaveRef.current;
    if (!next) return;
    queuedSaveRef.current = null;
    saveInFlightRef.current = true;
    try {
      await persistEditor(next.path, next.content);
    } finally {
      saveInFlightRef.current = false;
      if (queuedSaveRef.current) {
        void drainSaveQueue();
      }
    }
  }, [persistEditor]);

  const queueSave = useCallback((path: string, content: string) => {
    queuedSaveRef.current = { path, content };
    void drainSaveQueue();
  }, [drainSaveQueue]);

  useEffect(() => {
    if (editorValue === loadedValue) return;
    queueSave(selectedPathRef.current, editorValueRef.current);
  }, [editorValue, loadedValue, selectedPath, queueSave]);

  const saveFile = async () => {
    await persistEditor(selectedPathRef.current, editorValueRef.current);
    await loadFiles();
  };

  const value = useMemo(
    () => ({
      files,
      selectedPath,
      editorValue,
      setEditorValue,
      setEditorFocused,
      dirty,
      saving,
      error,
      generating,
      libraryHit,
      previewSrc,
      libraryTemplates,
      libraryLoading,
      snapshots,
      lastDiff,
      diffLoading,
      loadFiles,
      loadFile,
      saveFile,
      runPreview: applyDappUpdate,
      loadLibrary,
      applyLibraryTemplate,
      pinLibraryTemplate,
      unpinLibraryTemplate,
      deleteLibraryTemplate,
      loadHistory,
      undoSnapshot,
      showDiff,
    }),
    [
      files,
      selectedPath,
      editorValue,
      setEditorFocused,
      dirty,
      saving,
      error,
      generating,
      libraryHit,
      previewSrc,
      libraryTemplates,
      libraryLoading,
      snapshots,
      lastDiff,
      diffLoading,
      loadFiles,
      loadFile,
      saveFile,
      applyDappUpdate,
      loadLibrary,
      applyLibraryTemplate,
      pinLibraryTemplate,
      unpinLibraryTemplate,
      deleteLibraryTemplate,
      loadHistory,
      undoSnapshot,
      showDiff,
    ],
  );

  return (
    <DappContext.Provider value={value}>
      <div className={`dapp-root${interactive ? " dapp-root--interactive" : ""}`}>{children}</div>
    </DappContext.Provider>
  );
}
