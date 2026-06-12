import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import "./index.css";

type Theme = "light" | "dark";

type ChatSummary = {
  id: string;
  name: string;
  serverName: string;
  title: string;
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
  text?: string;
  model?: string;
  error?: string;
};

type KcodeState = {
  serverName?: string;
  git: { branch: string; status: string[]; commits: string[] };
  repo: { rustFiles: number; pythonFiles: number; tsFiles: number };
  runtime: { pid: number; eventTail: unknown[] };
};

function getInitialTheme(): Theme {
  const saved = localStorage.getItem("kcode-theme") as Theme | null;
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
  const [state, setState] = useState<KcodeState | null>(null);
  const [shutState, setShutState] = useState<"idle" | "confirm" | "down">("idle");
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("kcode-theme", theme);
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
    events.onmessage = () => scheduleRefreshFromServer();
    events.onerror = () => { /* EventSource reconnects automatically. */ };
    return () => {
      events.close();
      if (idleRefreshTimerRef.current !== null) window.clearTimeout(idleRefreshTimerRef.current);
    };
  }, [scheduleRefreshFromServer]);

  useEffect(() => {
    if (Date.now() < typingUntilRef.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, sending]);

  const newChat = () => {
    setActiveId(null);
    setActiveTitle("new chat");
    setMessages([]);
    setError(null);
  };

  const openChat = async (chat: ChatSummary) => {
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
            <button
              key={c.id}
              className={`chat-rail__item ${c.id === activeId ? "is-active" : ""}`}
              onClick={() => openChat(c)}
            >
              <span className="chat-rail__item-title">{c.title}</span>
              <span className="chat-rail__item-meta">{c.messageCount} msg{c.model && c.model !== "unknown" ? ` · ${c.model}` : ""}</span>
            </button>
          ))}
        </div>
      </aside>

      <main className="chat-main">
        <header className="chat-head">
          <div className="chat-head__left">
            <h1 className={`brand ${sending ? "brand--busy" : ""}`}>KCODE</h1>
            {!empty && (
              <span className="set-display">
                <span className="set-display__slash">/</span>
                <span className="set-display__name">{activeTitle}</span>
              </span>
            )}
          </div>
          <div className="chat-head__right">
            <button className="icon-btn" title="Live state" onClick={() => setShowInfo((v) => !v)}>ⓘ</button>
            <button className="icon-btn" title="Toggle theme" onClick={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}>
              {theme === "dark" ? "☀" : "☾"}
            </button>
            <button className="icon-btn icon-btn--power" title="Shut down kcode" onClick={() => setShutState("confirm")}>
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
            <span className={`brand brand--hero ${sending ? "brand--busy" : ""}`}>KCODE</span>
            <p className="chat-hero__sub">Chat with your kcode agent. Each conversation is its own session.</p>
          </div>
        )}

        {error && <div className="chat-error">{error}</div>}

        <div className="chat-input">
          <div className={`chat-input__inner ${sending ? "chat-input__inner--busy" : ""}`}>
            <textarea
              ref={inputRef}
              placeholder="Message kcode…"
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
            <h3>Shut down kcode?</h3>
            <p>This kills every kcode process — the agent, this web UI, and any running sessions. Run <code>kcode</code> in a terminal to start again.</p>
            <div className="modal__actions">
              <button className="btn btn--ghost" onClick={() => setShutState("idle")}>Cancel</button>
              <button className="btn btn--danger" onClick={shutdown}>Shut down</button>
            </div>
          </div>
        </div>
      )}

      {shutState === "down" && (
        <div className="shutdown-screen">
          <span className="brand brand--hero">KCODE</span>
          <p>kcode has shut down.</p>
          <p className="muted">Run <code>kcode</code> in a terminal to start it again.</p>
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
