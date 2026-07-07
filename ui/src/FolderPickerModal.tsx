import { useCallback, useEffect, useState } from "react";

type FsEntry = { name: string; path: string; kind: "dir" };

type BrowseResult = {
  path: string;
  parent: string | null;
  entries: FsEntry[];
};

type FolderPickerModalProps = {
  initialPath?: string;
  onSelect: (path: string) => void;
  onClose: () => void;
};

export default function FolderPickerModal({
  initialPath,
  onSelect,
  onClose,
}: FolderPickerModalProps) {
  const [browse, setBrowse] = useState<BrowseResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadBrowse = useCallback(async (path?: string) => {
    setLoading(true);
    setError(null);
    try {
      const url = path?.trim()
        ? `/api/fs/browse?path=${encodeURIComponent(path.trim())}`
        : "/api/fs/browse";
      const res = await fetch(url, { cache: "no-store" });
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || `Browse failed (${res.status})`);
      setBrowse(json as BrowseResult);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadBrowse(initialPath);
  }, [initialPath, loadBrowse]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="modal-scrim modal-scrim--nested" onClick={onClose}>
      <div className="modal modal--picker" onClick={(event) => event.stopPropagation()}>
        <h3>Select folder</h3>
        <p>Choose a project directory on this machine.</p>

        <div className="picker__toolbar">
          <button
            type="button"
            className="btn picker__up-btn"
            onClick={() => browse?.parent && void loadBrowse(browse.parent)}
            disabled={loading || !browse?.parent}
          >
            ↑ Up
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => void loadBrowse()}
            disabled={loading}
          >
            Home
          </button>
        </div>

        <div className="picker__path" title={browse?.path}>
          {loading ? "Loading…" : browse?.path ?? "—"}
        </div>

        <div className="picker__list" role="listbox" aria-label="Folders">
          {!loading && browse?.entries.length === 0 && (
            <p className="picker__empty">No folders here.</p>
          )}
          {browse?.entries.map((entry) => (
            <button
              key={entry.path}
              type="button"
              className="picker__item"
              onClick={() => void loadBrowse(entry.path)}
            >
              <span className="picker__item-name">{entry.name}</span>
            </button>
          ))}
        </div>

        {error && <p className="picker__error">{error}</p>}

        <div className="modal__actions">
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn btn--primary"
            onClick={() => browse?.path && onSelect(browse.path)}
            disabled={loading || !browse?.path}
          >
            Select folder
          </button>
        </div>
      </div>
    </div>
  );
}
