import { useDapp } from "./DappProvider";

export default function DappHistoryPanel() {
  const { snapshots, lastDiff, undoSnapshot, loadHistory, showDiff, diffLoading } = useDapp();

  return (
    <div className="dapp-history">
      <div className="dapp-history__head">
        <span className="dapp-history__title">History</span>
        <button type="button" className="dapp-sidebar__mini-btn" onClick={() => void loadHistory()} title="Refresh history">
          ↻
        </button>
      </div>
      {snapshots.length === 0 && <p className="dapp-library__empty">Snapshots appear after each dapp update.</p>}
      <ul className="dapp-history__list">
        {[...snapshots].reverse().map((snap) => (
          <li key={snap.turn} className="dapp-history__item">
            <span className="dapp-history__turn">Turn {snap.turn}</span>
            <div className="dapp-library__actions">
              <button type="button" className="dapp-library__btn" onClick={() => void showDiff(snap.turn)}>
                Diff
              </button>
              <button type="button" className="dapp-library__btn" onClick={() => void undoSnapshot(snap.turn)}>
                Undo
              </button>
            </div>
          </li>
        ))}
      </ul>
      {diffLoading && <p className="dapp-library__empty">Loading diff…</p>}
      {lastDiff && lastDiff.changedFiles.length > 0 && (
        <div className="dapp-diff">
          <p className="dapp-diff__summary">{lastDiff.summary}</p>
          {lastDiff.changedFiles.map((file) => (
            <details key={file} className="dapp-diff__file">
              <summary>{file}</summary>
              <pre>{lastDiff.files[file]?.unified || ""}</pre>
            </details>
          ))}
        </div>
      )}
    </div>
  );

}
