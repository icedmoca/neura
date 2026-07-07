import { useDapp } from "./DappProvider";

export default function DappLibraryPanel() {
  const {
    libraryTemplates,
    libraryLoading,
    loadLibrary,
    applyLibraryTemplate,
    pinLibraryTemplate,
    unpinLibraryTemplate,
    deleteLibraryTemplate,
  } = useDapp();

  return (
    <div className="dapp-library">
      <div className="dapp-library__head">
        <span className="dapp-library__title">Saved dapps</span>
        <button type="button" className="dapp-sidebar__mini-btn" onClick={() => void loadLibrary()} title="Refresh library">
          ↻
        </button>
      </div>
      {libraryLoading && <p className="dapp-library__empty">Loading…</p>}
      {!libraryLoading && libraryTemplates.length === 0 && (
        <p className="dapp-library__empty">Chat to build your first saved dapp.</p>
      )}
      <ul className="dapp-library__list">
        {libraryTemplates.map((template) => (
          <li key={template.id} className="dapp-library__item">
            <div className="dapp-library__meta">
              <span className="dapp-library__name">
                {template.pinned ? "★ " : ""}
                {template.title || template.slug || template.id}
              </span>
              {template.category && (
                <span className="dapp-library__cat">{template.category}</span>
              )}
              <span className="dapp-library__uses">{template.useCount ?? 0} uses</span>
            </div>
            <div className="dapp-library__actions">
              <button type="button" className="dapp-library__btn" onClick={() => void applyLibraryTemplate(template.id)}>
                Use
              </button>
              <button
                type="button"
                className="dapp-library__btn"
                onClick={() => void (template.pinned ? unpinLibraryTemplate(template.id) : pinLibraryTemplate(template.id))}
              >
                {template.pinned ? "Unpin" : "Pin"}
              </button>
              <button type="button" className="dapp-library__btn dapp-library__btn--danger" onClick={() => void deleteLibraryTemplate(template.id)}>
                Del
              </button>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
