import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import "./index.css";

type Theme = "light" | "dark";

type ChatSummary = {
  id: string;
  name: string;
  serverName: string;
  title: string;
  titleLocked?: boolean;
  titleSource?: string;
  model: string | null;
  updatedAt: string | number | null;
  messageCount: number;
};

type ChatMessage = { role: "user" | "assistant"; text: string; tools?: string[] };

type ChatTurnResult = {
  session_id?: string;
  name?: string;
  serverName?: string;
  title?: string;
  titleLocked?: boolean;
  titleSource?: string;
  text?: string;
  model?: string;
  error?: string;
};

type NeuraState = {
  serverName?: string;
  git: { branch: string; status: string[]; commits: string[] };
  repo: { rustFiles: number; pythonFiles: number; tsFiles: number };
  runtime: { pid: number; eventTail: unknown[] };
};

function getInitialTheme(): Theme {
  const saved = localStorage.getItem("neura-theme") as Theme | null;
  if (saved === "light" || saved === "dark") return saved;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

const SendIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M12 19V5M5 12l7-7 7 7" />
  </svg>
);

const PowerIcon = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M12 2v10" /><path d="M18.4 6.6a9 9 0 1 1-12.8 0" />
  </svg>
);

function App() {
  const [theme, setTheme] = useState<Theme>(getInitialTheme);
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [serverName, setServerName] = useState("");
  const [activeId, setActiveId] = useState<string | null>(null);
  const [activeTitle, setActiveTitle] = useState("new chat");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const typingUntilRef = useRef(0);
  const pendingRefreshRef = useRef(false);
  const idleRefreshTimerRef = useRef<number | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showInfo, setShowInfo] = useState(false);
  const [state, setState] = useState<NeuraState | null>(null);
  const [shutState, setShutState] = useState<"idle" | "confirm" | "down">("idle");
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("neura-theme", theme);
  }, [theme]);

  const loadChats = useCallback(async () => {
    try {
      const res = await fetch("/api/chats", { cache: "no-store" });
      const json = await res.json();
      setChats(json.chats ?? []);
      setServerName(json.serverName ?? "");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const refreshActiveChat = useCallback(async () => {
    if (!activeId) return;
    try {
      const res = await fetch(`/api/chats/${activeId}`, { cache: "no-store" });
      const json = await res.json();
      setMessages(json.messages ?? []);
    } catch { /* non-critical live refresh */ }
  }, [activeId]);

  const loadState = useCallback(async () => {
    try {
      const res = await fetch("/api/state", { cache: "no-store" });
      setState(await res.json());
    } catch { /* non-critical */ }
  }, []);

  const refreshFromServer = useCallback(() => {
    pendingRefreshRef.current = false;
    void loadChats();
    void loadState();
    void refreshActiveChat();
  }, [loadChats, loadState, refreshActiveChat]);

  const scheduleRefreshFromServer = useCallback(() => {
    const delay = Math.max(0, typingUntilRef.current - Date.now());
    pendingRefreshRef.current = true;
    if (idleRefreshTimerRef.current !== null) {
      window.clearTimeout(idleRefreshTimerRef.current);
    }
    idleRefreshTimerRef.current = window.setTimeout(() => {
      idleRefreshTimerRef.current = null;
      if (pendingRefreshRef.current) refreshFromServer();
    }, delay || 75);
  }, [refreshFromServer]);

  const markTyping = useCallback(() => {
    typingUntilRef.current = Date.now() + 550;
  }, []);

  useEffect(() => { refreshFromServer(); }, [refreshFromServer]);

  useEffect(() => {
    const events = new EventSource("/api/events");
    events.onmessage = (event) => {
      try {
        const payload = JSON.parse(event.data) as { type?: string; session_id?: string; title?: string };
        if (payload.type === "chat_title_updated" && payload.session_id && payload.title) {
          setChats((prev) =>
            prev.map((chat) =>
              chat.id === payload.session_id
                ? { ...chat, title: payload.title as string, titleLocked: false, titleSource: "auto" }
                : chat,
            ),
          );
          if (activeId === payload.session_id) {
            setActiveTitle(payload.title);
          }
        }
      } catch {
        /* ignore malformed events */
      }
      scheduleRefreshFromServer();
    };
    events.onerror = () => { /* EventSource reconnects automatically. */ };
    return () => {
      events.close();
      if (idleRefreshTimerRef.current !== null) window.clearTimeout(idleRefreshTimerRef.current);
    };
  }, [scheduleRefreshFromServer, activeId]);

  useEffect(() => {
    if (Date.now() < typingUntilRef.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, sending]);

  useEffect(() => {
    if (renamingId && renameInputRef.current) {
      renameInputRef.current.focus();
      renameInputRef.current.select();
    }
  }, [renamingId]);

  const startRename = (chat: ChatSummary) => {
    setRenamingId(chat.id);
    setRenameDraft(chat.title);
    setError(null);
  };

  const cancelRename = () => {
    setRenamingId(null);
    setRenameDraft("");
  };

  const commitRename = useCallback(async () => {
    const sessionId = renamingId;
    const title = renameDraft.trim();
    if (!sessionId || !title) {
      cancelRename();
      return;
    }
    try {
      const res = await fetch(`/api/chats/${sessionId}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title, lock: true }),
      });
      const json = await res.json();
      if (!res.ok) {
        throw new Error(json.error || `Rename failed (${res.status})`);
      }
      setChats((prev) =>
        prev.map((chat) =>
          chat.id === sessionId
            ? { ...chat, title: json.title ?? title, titleLocked: true, titleSource: "user" }
            : chat,
        ),
      );
      if (activeId === sessionId) {
        setActiveTitle(json.title ?? title);
      }
      cancelRename();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [renamingId, renameDraft, activeId]);

  const newChat = () => {
    setActiveId(null);
    setActiveTitle("new chat");
    setMessages([]);
    setError(null);
  };

  const openChat = async (chat: ChatSummary) => {
    if (renamingId) cancelRename();
    setActiveId(chat.id);
    setActiveTitle(chat.title);
    setError(null);
    try {
      const res = await fetch(`/api/chats/${chat.id}`, { cache: "no-store" });
      const json = await res.json();
      setMessages(json.messages ?? []);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const send = useCallback(async () => {
    const textarea = inputRef.current;
    const text = textarea?.value.trim() ?? "";
    if (!text || sending) return;
    if (textarea) textarea.value = "";
    setError(null);
    setMessages((m) => [...m, { role: "user", text }]);
    setSending(true);
    try {
      const res = await fetch("/api/chat", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ session_id: activeId, message: text }),
      });
      const json = (await res.json()) as ChatTurnResult;
      if (json.error) {
        setError(json.error);
      } else {
        if (json.session_id) setActiveId(json.session_id);
        if (json.title) setActiveTitle(json.title);
        setMessages((m) => [...m, { role: "assistant", text: json.text ?? "" }]);
        loadChats();
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSending(false);
    }
  }, [activeId, sending]);

  const onKey = useCallback((e: KeyboardEvent<HTMLTextAreaElement>) => {
    markTyping();
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  }, [markTyping, send]);

  const shutdown = async () => {
    setShutState("down");
    try {
      // The server kills itself right after replying, so the connection may drop.
      await fetch("/api/shutdown", { method: "POST" });
    } catch { /* expected — the server is going away */ }
  };

  const empty = messages.length === 0 && !sending;
  const showHeaderRename = Boolean(renamingId && renamingId === activeId && !empty);

  return (
    <div className={`chat-wrap chat-wrap--signed-in ${empty ? "chat-wrap--empty" : ""}`}>
      <div className="bg-layer bg-layer--on" style={{ backgroundImage: "url(/bg/neura-bg.jpg)" }} />

      <aside className="chat-rail" aria-label="Chats">
        <div className="chat-rail__top">
          <span className="chat-rail__label">{serverName ? `server · ${serverName}` : "chats"}</span>
          <button className="chat-rail__new" onClick={newChat}>+ New</button>
        </div>
        <div className="chat-rail__scroll">
          {chats.length === 0 && <p className="chat-rail__empty">No chats yet. Start one below.</p>}
          {chats.map((c) => (
            <div
              key={c.id}
              className={`chat-rail__item-wrap ${c.id === activeId ? "is-active" : ""}`}
            >
              {renamingId === c.id && !showHeaderRename ? (
                <div className="chat-rail__rename">
                  <input
                    ref={renameInputRef}
                    className="chat-rail__rename-input"
                    value={renameDraft}
                    onChange={(e) => setRenameDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        void commitRename();
                      } else if (e.key === "Escape") {
                        e.preventDefault();
                        cancelRename();
                      }
                    }}
                    onBlur={() => void commitRename()}
                  />
                </div>
              ) : (
                <button
                  className={`chat-rail__item ${c.id === activeId ? "is-active" : ""}`}
                  onClick={() => openChat(c)}
                  onDoubleClick={(e) => {
                    e.preventDefault();
                    startRename(c);
                  }}
                >
                  <span className="chat-rail__item-title" title="Double-click to rename">{c.title}</span>
                  <span className="chat-rail__item-meta">
                    {c.messageCount} msg{c.model && c.model !== "unknown" ? ` · ${c.model}` : ""}
                    {c.titleLocked ? " · pinned" : ""}
                  </span>
                </button>
              )}
            </div>
          ))}
        </div>
      </aside>

      <main className="chat-main">
        <header className="chat-head">
          <div className="chat-head__left">
            <h1 className={`brand ${sending ? "brand--busy" : ""}`}>NEURA</h1>
            {!empty && (
              <span className="set-display">
                <span className="set-display__slash">/</span>
                {activeId && showHeaderRename ? (
                  <input
                    ref={renameInputRef}
                    className="set-display__rename"
                    value={renameDraft}
                    onChange={(e) => setRenameDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        void commitRename();
                      } else if (e.key === "Escape") {
                        e.preventDefault();
                        cancelRename();
                      }
                    }}
                    onBlur={() => void commitRename()}
                  />
                ) : (
                  <button
                    type="button"
                    className="set-display__name set-display__name--button"
                    title="Click to rename chat"
                    onClick={() => {
                      const chat = chats.find((c) => c.id === activeId);
                      if (chat) startRename(chat);
                    }}
                  >
                    {activeTitle}
                  </button>
                )}
              </span>
            )}
          </div>
          <div className="chat-head__right">
            <button className="icon-btn" title="Live state" onClick={() => setShowInfo((v) => !v)}>ⓘ</button>
            <button className="icon-btn" title="Toggle theme" onClick={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}>
              {theme === "dark" ? "☀" : "☾"}
            </button>
            <button className="icon-btn icon-btn--power" title="Shut down neura" onClick={() => setShutState("confirm")}>
              <PowerIcon />
            </button>
          </div>
        </header>

        <div className="chat-log" ref={scrollRef}>
          {messages.map((m, i) => (
            <div key={i} className={`chat-msg chat-msg--${m.role}`}>
              {m.role === "assistant" && <span className="chat-msg__who">{activeTitle}</span>}
              <div className="chat-msg__body">
                {m.tools && m.tools.length > 0 && <div className="chat-msg__tools">🔧 {m.tools.join(", ")}</div>}
                {m.text || <em className="muted">…</em>}
              </div>
            </div>
          ))}
          {sending && (
            <div className="chat-msg chat-msg--assistant">
              <span className="chat-msg__who">{activeTitle}</span>
              <div className="chat-msg__body chat-msg__body--thinking">thinking…</div>
            </div>
          )}
        </div>

        {empty && (
          <div className="chat-hero">
            <span className={`brand brand--hero ${sending ? "brand--busy" : ""}`}>NEURA</span>
            <p className="chat-hero__sub">Chat with your neura agent. Each conversation is its own session.</p>
          </div>
        )}

        {error && <div className="chat-error">{error}</div>}

        <div className="chat-input">
          <div className={`chat-input__inner ${sending ? "chat-input__inner--busy" : ""}`}>
            <textarea
              ref={inputRef}
              placeholder="Message neura…"
              onFocus={markTyping}
              onInput={markTyping}
              onKeyDown={onKey}
              rows={1}
            />
            <button className="send-btn" onClick={send} disabled={sending} title="Send">
              <SendIcon />
            </button>
          </div>
        </div>
      </main>

      {shutState === "confirm" && (
        <div className="modal-scrim" onClick={() => setShutState("idle")}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal__icon"><PowerIcon /></div>
            <h3>Shut down neura?</h3>
            <p>This kills every neura process — the agent, this web UI, and any running sessions. Run <code>neura</code> in a terminal to start again.</p>
            <div className="modal__actions">
              <button className="btn btn--ghost" onClick={() => setShutState("idle")}>Cancel</button>
              <button className="btn btn--danger" onClick={shutdown}>Shut down</button>
            </div>
          </div>
        </div>
      )}

      {shutState === "down" && (
        <div className="shutdown-screen">
          <span className="brand brand--hero">NEURA</span>
          <p>neura has shut down.</p>
          <p className="muted">Run <code>neura</code> in a terminal to start it again.</p>
        </div>
      )}

      {showInfo && state && (
        <aside className="info-drawer" onClick={() => setShowInfo(false)}>
          <div className="info-card" onClick={(e) => e.stopPropagation()}>
            <div className="info-card__head">
              <h3>Live state</h3>
              <button className="icon-btn" onClick={() => setShowInfo(false)}>✕</button>
            </div>
            <dl className="info-grid">
              <div><dt>server</dt><dd>{state.serverName ?? serverName}</dd></div>
              <div><dt>git branch</dt><dd>{state.git?.branch}</dd></div>
              <div><dt>uncommitted</dt><dd>{state.git?.status?.length ?? 0}</dd></div>
              <div><dt>rust files</dt><dd>{state.repo?.rustFiles}</dd></div>
              <div><dt>python files</dt><dd>{state.repo?.pythonFiles}</dd></div>
              <div><dt>ts files</dt><dd>{state.repo?.tsFiles}</dd></div>
              <div><dt>server pid</dt><dd>{state.runtime?.pid}</dd></div>
              <div><dt>chats</dt><dd>{chats.length}</dd></div>
            </dl>
          </div>
        </aside>
      )}
    </div>
  );
}

export default App;
