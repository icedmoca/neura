import { useCallback, useEffect, useRef, useMemo, useState } from "react";
import type { KeyboardEvent } from "react";
import MarkdownMessage from "./MarkdownMessage";
import { DappProvider } from "./dapp/DappProvider";
import DappPreview from "./dapp/DappPreview";
import DappSidebar from "./dapp/DappSidebar";
import FolderPickerModal from "./FolderPickerModal";
import CognitionPanel from "./CognitionPanel";
import "./index.css";

type ChatSummary = {
  id: string;
  name: string;
  serverName: string;
  title: string;
  titleLocked?: boolean;
  titleSource?: string;
  model: string | null;
  workingDir?: string | null;
  updatedAt: string | number | null;
  messageCount: number;
};

type ProjectSummary = {
  id: string;
  path: string;
  name: string;
  chatCount: number;
  pinned?: boolean;
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

type SpeechRecognitionAlternativeLike = { transcript: string };

type SpeechRecognitionResultLike = {
  isFinal: boolean;
  length: number;
  item: (index: number) => SpeechRecognitionAlternativeLike;
  [index: number]: SpeechRecognitionAlternativeLike;
};

type SpeechRecognitionEventLike = {
  resultIndex: number;
  results: {
    length: number;
    item: (index: number) => SpeechRecognitionResultLike;
    [index: number]: SpeechRecognitionResultLike;
  };
};

type SpeechRecognitionErrorEventLike = { error: string };

type SpeechRecognitionInstance = {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  onresult: ((event: SpeechRecognitionEventLike) => void) | null;
  onerror: ((event: SpeechRecognitionErrorEventLike) => void) | null;
  onend: (() => void) | null;
  start: () => void;
  stop: () => void;
  abort: () => void;
};

type SpeechRecognitionCtor = new () => SpeechRecognitionInstance;

function getSpeechRecognitionCtor(): SpeechRecognitionCtor | null {
  const w = window as Window & {
    SpeechRecognition?: SpeechRecognitionCtor;
    webkitSpeechRecognition?: SpeechRecognitionCtor;
  };
  return w.SpeechRecognition ?? w.webkitSpeechRecognition ?? null;
}

function voiceRequestedOnLoad(): boolean {
  return new URLSearchParams(window.location.search).get("voice") === "1";
}

function mergeServerMessages(server: ChatMessage[], local: ChatMessage[]): ChatMessage[] {
  const serverUserCount = server.filter((m) => m.role === "user").length;
  const localUserCount = local.filter((m) => m.role === "user").length;
  if (localUserCount <= serverUserCount) return server;

  const extraUsers: ChatMessage[] = [];
  let remaining = localUserCount - serverUserCount;
  for (let i = local.length - 1; i >= 0 && remaining > 0; i--) {
    if (local[i].role === "user") {
      extraUsers.unshift(local[i]);
      remaining--;
    }
  }
  return [...server, ...extraUsers];
}

function projectBadge(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  const word = parts[0] ?? "?";
  return word.slice(0, 2).toUpperCase();
}

function focusInput(inputRef: React.RefObject<HTMLTextAreaElement | null>) {
  window.setTimeout(() => inputRef.current?.focus(), 0);
}

function isVoiceHoldBlocked(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return true;
  return Boolean(
    target.closest(
      "input, textarea, button, select, a, label, .modal-scrim, .info-drawer, .sidebar__rename-input",
    ),
  );
}

const SendIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M12 19V5M5 12l7-7 7 7" />
  </svg>
);

const CognitionIcon = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    <circle cx="12" cy="5" r="2" /><circle cx="5" cy="17" r="2" /><circle cx="19" cy="17" r="2" />
    <path d="M12 7v4m0 0-5.3 4.4M12 11l5.3 4.4" />
  </svg>
);

const StopIcon = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden>
    <rect x="6" y="6" width="12" height="12" />
  </svg>
);

const PowerIcon = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M12 2v10" /><path d="M18.4 6.6a9 9 0 1 1-12.8 0" />
  </svg>
);

const CopyIcon = () => (
  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    <rect x="9" y="9" width="13" height="13" />
    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
  </svg>
);

const CheckIcon = () => (
  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    <path d="M20 6 9 17l-5-5" />
  </svg>
);

function App() {
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [activeProjectPath, setActiveProjectPath] = useState<string | null>(null);
  const [activeProjectName, setActiveProjectName] = useState("");
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [serverName, setServerName] = useState("");
  const [activeId, setActiveId] = useState<string | null>(null);
  const [activeTitle, setActiveTitle] = useState("new chat");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const finalTranscriptRef = useRef("");
  const recognitionRef = useRef<SpeechRecognitionInstance | null>(null);
  const voiceActiveRef = useRef(false);
  const voiceWantedRef = useRef(voiceRequestedOnLoad());
  const pendingSendRef = useRef<string[]>([]);
  const pumpQueueRef = useRef(false);
  const activeIdRef = useRef<string | null>(null);
  const activeProjectPathRef = useRef<string | null>(null);
  const skipAutoProjectRef = useRef(false);
  const sendingRef = useRef(false);
  const startVoiceRef = useRef<() => void>(() => {});
  const stopVoiceRef = useRef<() => void>(() => {});
  const typingUntilRef = useRef(0);
  const pendingRefreshRef = useRef(false);
  const idleRefreshTimerRef = useRef<number | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showInfo, setShowInfo] = useState(false);
  const [showCognition, setShowCognition] = useState(false);
  const [state, setState] = useState<NeuraState | null>(null);
  const [shutState, setShutState] = useState<"idle" | "confirm" | "down">("idle");
  const [showAddProject, setShowAddProject] = useState(false);
  const [showFolderPicker, setShowFolderPicker] = useState(false);
  const [projectPathDraft, setProjectPathDraft] = useState("");
  const [projectNameDraft, setProjectNameDraft] = useState("");
  const [addingProject, setAddingProject] = useState(false);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [voiceActive, setVoiceActive] = useState(false);
  const [voiceHoldArmed, setVoiceHoldArmed] = useState(false);
  const [voiceStatus, setVoiceStatus] = useState<string | null>(null);
  const [queuedCount, setQueuedCount] = useState(0);
  const [copiedMessageKey, setCopiedMessageKey] = useState<string | null>(null);
  const [chatHidden, setChatHidden] = useState(false);
  const [dappRefreshToken, setDappRefreshToken] = useState(0);
  const [dappGenerating, setDappGenerating] = useState(false);
  const [dappLibraryHit, setDappLibraryHit] = useState<string | null>(null);
  const [sidebarMode, setSidebarMode] = useState<"dapp" | "chats">("dapp");
  const [latentWords, setLatentWords] = useState<string[]>([]);
  const [latentPhase, setLatentPhase] = useState<string | null>(null);
  const [lastThought, setLastThought] = useState<{ text: string; words: string[] } | null>(null);
  const latentTextRef = useRef<string>("");
  // Per-chat history of the observer's polished thoughts (newest first).
  const [thoughtLog, setThoughtLog] = useState<{ text: string; at: number }[]>([]);
  // Live turn activity from /api/chat/stream: tool chips + reasoning stream.
  const [liveTools, setLiveTools] = useState<{ id: string; name: string; status: "running" | "done" | "error" }[]>([]);
  const [liveReasoning, setLiveReasoning] = useState("");
  const liveReasoningRef = useRef("");
  const currentSendRef = useRef<string | null>(null);
  // Latest readable narration sentence from subtext frames (word chips are
  // only shown when a frame carries true latent words and no sentence).
  const [latentLine, setLatentLine] = useState("");
  const [turnNote, setTurnNote] = useState<string | null>(null);
  const sendingSessionRef = useRef<string | null>(null);
  const subtextConfigRef = useRef<{ endpoint: string; enabled: boolean; model?: string } | null>(null);
  const subtextAbortRef = useRef<AbortController | null>(null);
  const messagesRef = useRef<ChatMessage[]>([]);
  const copyResetTimerRef = useRef<number | null>(null);
  const copyHoverLockRef = useRef<string | null>(null);
  const chatHiddenRef = useRef(false);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const projectPathInputRef = useRef<HTMLInputElement>(null);
  const projectPathEditedRef = useRef(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const speechCtor = useMemo(() => getSpeechRecognitionCtor(), []);
  const voiceSupported = speechCtor !== null;
  const activeProjectId = useMemo(
    () => projects.find((project) => project.path === activeProjectPath)?.id ?? null,
    [projects, activeProjectPath],
  );

  useEffect(() => {
    chatHiddenRef.current = chatHidden;
  }, [chatHidden]);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", "dark");
    localStorage.setItem("neura-theme", "dark");
  }, []);

  useEffect(() => () => {
    if (copyResetTimerRef.current !== null) {
      window.clearTimeout(copyResetTimerRef.current);
    }
  }, []);

  useEffect(() => {
    activeIdRef.current = activeId;
  }, [activeId]);

  useEffect(() => {
    activeProjectPathRef.current = activeProjectPath;
  }, [activeProjectPath]);

  useEffect(() => {
    sendingRef.current = sending;
  }, [sending]);

  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const res = await fetch("/api/subtext-config", { cache: "no-store" });
        const cfg = await res.json();
        if (!cancelled && cfg && typeof cfg.endpoint === "string") {
          subtextConfigRef.current = cfg;
        }
      } catch {
        /* observer is best-effort; ignore */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const loadProjects = useCallback(async () => {
    try {
      const res = await fetch("/api/projects", { cache: "no-store" });
      const json = await res.json();
      setProjects(json.projects ?? []);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const loadChats = useCallback(async (projectPath: string | null) => {
    if (!projectPath) {
      setChats([]);
      return;
    }
    try {
      const res = await fetch(
        `/api/chats?project=${encodeURIComponent(projectPath)}`,
        { cache: "no-store" },
      );
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
      const serverMessages = (json.messages ?? []) as ChatMessage[];
      if (sendingRef.current || pendingSendRef.current.length > 0) {
        setMessages((prev) => mergeServerMessages(serverMessages, prev));
      } else {
        setMessages(serverMessages);
      }
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
    void loadProjects();
    void loadChats(activeProjectPathRef.current);
    void loadState();
    void refreshActiveChat();
  }, [loadProjects, loadChats, loadState, refreshActiveChat]);

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

  const applyTranscriptToInput = useCallback((interim: string) => {
    const textarea = inputRef.current;
    if (!textarea) return;
    const prefix = finalTranscriptRef.current;
    const spacer = prefix && interim ? " " : "";
    textarea.value = `${prefix}${spacer}${interim}`.trimStart();
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    markTyping();
  }, [markTyping]);

  const stopVoice = useCallback(() => {
    voiceActiveRef.current = false;
    setVoiceActive(false);
    const recognition = recognitionRef.current;
    recognitionRef.current = null;
    recognition?.stop();
  }, []);

  const startVoice = useCallback(() => {
    if (!speechCtor) return;

    stopVoice();
    voiceActiveRef.current = true;
    setVoiceActive(true);
    setVoiceStatus(
      sendingRef.current
        ? "Listening — release when done."
        : "Listening — release to finish dictating.",
    );

    const recognition = new speechCtor();
    recognition.continuous = true;
    recognition.interimResults = true;
    recognition.lang = navigator.language || "en-US";

    recognition.onresult = (event) => {
      let interim = "";
      for (let i = event.resultIndex; i < event.results.length; i++) {
        const result = event.results[i];
        const text = (result[0] ?? result.item(0)).transcript.trim();
        if (!text) continue;
        if (result.isFinal) {
          const base = finalTranscriptRef.current.trim();
          finalTranscriptRef.current = base ? `${base} ${text}` : text;
        } else {
          interim += text;
        }
      }
      applyTranscriptToInput(interim.trim());
    };

    recognition.onerror = (event) => {
      if (event.error === "not-allowed") {
        setVoiceStatus("Microphone permission denied.");
        stopVoice();
        return;
      }
      if (event.error !== "aborted" && event.error !== "no-speech") {
        setVoiceStatus(`Voice error: ${event.error}`);
      }
    };

    recognition.onend = () => {
      if (!voiceActiveRef.current) {
        setVoiceActive(false);
        return;
      }
      try {
        recognition.start();
      } catch {
        voiceActiveRef.current = false;
        setVoiceActive(false);
      }
    };

    recognitionRef.current = recognition;
    try {
      recognition.start();
    } catch (err) {
      voiceActiveRef.current = false;
      setVoiceActive(false);
      setVoiceStatus(err instanceof Error ? err.message : "Could not start voice input.");
    }
  }, [applyTranscriptToInput, speechCtor, stopVoice]);

  startVoiceRef.current = startVoice;
  stopVoiceRef.current = stopVoice;

  useEffect(() => {
    if (!voiceSupported) return;

    const DRAG_PX = 8;
    const HOLD_MS = 220;

    let startX = 0;
    let startY = 0;
    let activePointerId: number | null = null;
    let dragged = false;
    let holdTimer: number | null = null;
    let startedFromHold = false;

    const clearHoldTimer = () => {
      if (holdTimer !== null) {
        window.clearTimeout(holdTimer);
        holdTimer = null;
      }
    };

    const resetHold = () => {
      clearHoldTimer();
      activePointerId = null;
      dragged = false;
      startedFromHold = false;
      setVoiceHoldArmed(false);
    };

    const onPointerDown = (event: PointerEvent) => {
      if (event.button !== 0) return;
      if (chatHiddenRef.current) return;
      if (isVoiceHoldBlocked(event.target)) return;
      if (activePointerId !== null) return;

      activePointerId = event.pointerId;
      startX = event.clientX;
      startY = event.clientY;
      dragged = false;
      startedFromHold = false;
      setVoiceHoldArmed(true);

      holdTimer = window.setTimeout(() => {
        holdTimer = null;
        if (dragged || activePointerId === null) return;
        startedFromHold = true;
        setVoiceStatus("Listening — release to finish dictating.");
        startVoiceRef.current();
      }, HOLD_MS);
    };

    const onPointerMove = (event: PointerEvent) => {
      if (activePointerId === null || event.pointerId !== activePointerId) return;
      const dx = event.clientX - startX;
      const dy = event.clientY - startY;
      if (Math.hypot(dx, dy) >= DRAG_PX) {
        dragged = true;
        clearHoldTimer();
        setVoiceHoldArmed(false);
      }
    };

    const onPointerEnd = (event: PointerEvent) => {
      if (activePointerId === null || event.pointerId !== activePointerId) return;
      clearHoldTimer();
      if (startedFromHold) {
        stopVoiceRef.current();
        setVoiceStatus(null);
      }
      resetHold();
    };

    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("pointermove", onPointerMove, true);
    window.addEventListener("pointerup", onPointerEnd, true);
    window.addEventListener("pointercancel", onPointerEnd, true);

    return () => {
      resetHold();
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("pointermove", onPointerMove, true);
      window.removeEventListener("pointerup", onPointerEnd, true);
      window.removeEventListener("pointercancel", onPointerEnd, true);
    };
  }, [voiceSupported]);

  useEffect(() => {
    if (!voiceActive) return;
    if (sending) {
      const extra = queuedCount > 0 ? ` (${queuedCount} queued)` : "";
      setVoiceStatus(`Listening — release when done${extra}.`);
    } else {
      setVoiceStatus((prev) => {
        if (prev?.startsWith("Voice error") || prev === "Microphone permission denied.") return prev;
        return "Listening — release to finish dictating.";
      });
    }
  }, [sending, voiceActive, queuedCount]);

  useEffect(() => {
    if (!voiceWantedRef.current) return;
    if (voiceSupported) {
      startVoiceRef.current();
    } else {
      setVoiceStatus("Voice input is not supported in this browser.");
    }
  }, [voiceSupported]);

  useEffect(() => () => {
    voiceActiveRef.current = false;
    recognitionRef.current?.abort();
    recognitionRef.current = null;
  }, []);

  useEffect(() => { refreshFromServer(); }, [refreshFromServer]);

  useEffect(() => {
    if (skipAutoProjectRef.current || activeProjectPath || projects.length === 0) return;
    const first = projects[0];
    setActiveProjectPath(first.path);
    setActiveProjectName(first.name);
  }, [projects, activeProjectPath]);

  useEffect(() => {
    void loadChats(activeProjectPath);
  }, [activeProjectPath, loadChats]);

  useEffect(() => {
    if (!sending || !activeId) return;
    const timer = window.setInterval(() => {
      void refreshActiveChat();
    }, 450);
    return () => window.clearInterval(timer);
  }, [sending, activeId, refreshActiveChat]);

  useEffect(() => {
    const events = new EventSource("/api/events");
    events.onmessage = (event) => {
      try {
        const payload = JSON.parse(event.data) as {
          type?: string;
          session_id?: string;
          title?: string;
          project_path?: string;
        };
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
        if (
          payload.type === "dapp_changed"
          && payload.project_path
          && payload.project_path === activeProjectPathRef.current
        ) {
          setDappRefreshToken((value) => value + 1);
          if ((payload as { reused?: boolean }).reused) {
            setDappGenerating(false);
            const title = (payload as { templateTitle?: string }).templateTitle;
            if (title) {
              setDappLibraryHit(title);
              window.setTimeout(() => setDappLibraryHit(null), 4000);
            }
          }
        }
        if (
          payload.type === "dapp_generating"
          && payload.project_path
          && payload.project_path === activeProjectPathRef.current
        ) {
          setDappGenerating(true);
        }
        if (
          payload.type === "dapp_generation_done"
          && payload.project_path
          && payload.project_path === activeProjectPathRef.current
        ) {
          setDappGenerating(false);
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

  useEffect(() => {
    if (showAddProject && projectPathInputRef.current) {
      projectPathInputRef.current.focus();
      projectPathInputRef.current.select();
    }
  }, [showAddProject]);

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

  const beginNewChat = useCallback(() => {
    setActiveId(null);
    setActiveTitle("new chat");
    setMessages([]);
    setError(null);
    setLastThought(null);
    focusInput(inputRef);
  }, []);

  const exitProject = useCallback(() => {
    if (renamingId) cancelRename();
    skipAutoProjectRef.current = true;
    setChatHidden(false);
    setSidebarMode("dapp");
    setActiveProjectPath(null);
    setActiveProjectName("");
    setActiveId(null);
    setActiveTitle("new chat");
    setMessages([]);
    setError(null);
    setLastThought(null);
  }, [renamingId]);

  const selectProject = useCallback((project: ProjectSummary) => {
    if (renamingId) cancelRename();
    skipAutoProjectRef.current = false;
    setChatHidden(false);
    setSidebarMode("dapp");
    setActiveProjectPath(project.path);
    setActiveProjectName(project.name);
    setActiveId(null);
    setActiveTitle("new chat");
    setMessages([]);
    setError(null);
    setLastThought(null);
    focusInput(inputRef);
  }, [renamingId]);

  const newChat = () => {
    if (!activeProjectPath) return;
    beginNewChat();
  };

  const toggleChatHidden = () => {
    setChatHidden((hidden) => !hidden);
  };

  const openAddProject = () => {
    projectPathEditedRef.current = false;
    setProjectNameDraft("");
    setShowAddProject(true);
    setError(null);
    void (async () => {
      try {
        const res = await fetch("/api/projects/suggest-path", { cache: "no-store" });
        const json = await res.json();
        if (!res.ok) throw new Error(json.error || "Could not suggest project path");
        if (!projectPathEditedRef.current) {
          setProjectPathDraft(json.path ?? "");
        }
      } catch {
        setProjectPathDraft("");
      }
    })();
  };

  const syncSuggestedProjectPath = useCallback(async (name: string) => {
    if (projectPathEditedRef.current) return;
    try {
      const query = name.trim() ? `?name=${encodeURIComponent(name.trim())}` : "";
      const res = await fetch(`/api/projects/suggest-path${query}`, { cache: "no-store" });
      const json = await res.json();
      if (!res.ok) throw new Error(json.error || "Could not suggest project path");
      if (!projectPathEditedRef.current) {
        setProjectPathDraft(json.path ?? "");
      }
    } catch {
      /* keep current draft */
    }
  }, []);

  useEffect(() => {
    if (!showAddProject || projectPathEditedRef.current) return;
    const timer = window.setTimeout(() => {
      void syncSuggestedProjectPath(projectNameDraft);
    }, 180);
    return () => window.clearTimeout(timer);
  }, [projectNameDraft, showAddProject, syncSuggestedProjectPath]);

  const closeAddProject = () => {
    setShowAddProject(false);
    setShowFolderPicker(false);
    setProjectPathDraft("");
    setProjectNameDraft("");
    setAddingProject(false);
    projectPathEditedRef.current = false;
  };

  const submitAddProject = async () => {
    const path = projectPathDraft.trim();
    if (!path) {
      setError("Project path is required.");
      return;
    }
    setAddingProject(true);
    setError(null);
    try {
      const res = await fetch("/api/projects", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          path,
          name: projectNameDraft.trim() || undefined,
        }),
      });
      const json = await res.json();
      if (!res.ok) {
        throw new Error(json.error || `Add project failed (${res.status})`);
      }
      const project = json as ProjectSummary;
      await loadProjects();
      selectProject(project);
      closeAddProject();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setAddingProject(false);
    }
  };

  const ensureActiveProject = useCallback(async (): Promise<string> => {
    const existing = activeProjectPathRef.current;
    if (existing) return existing;

    const adoptProject = (project: ProjectSummary) => {
      skipAutoProjectRef.current = false;
      activeProjectPathRef.current = project.path;
      setActiveProjectPath(project.path);
      setActiveProjectName(project.name);
      void loadProjects();
      return project.path;
    };

    try {
      const autoRes = await fetch("/api/projects", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ auto: true }),
      });
      const autoJson = await autoRes.json();
      if (autoRes.ok) {
        return adoptProject(autoJson as ProjectSummary);
      }
    } catch {
      /* try fallback below */
    }

    const workspaceRes = await fetch("/api/workspace", { cache: "no-store" });
    const workspaceJson = await workspaceRes.json();
    if (!workspaceRes.ok) {
      throw new Error(workspaceJson.error || `Failed to resolve workspace (${workspaceRes.status})`);
    }

    const createRes = await fetch("/api/projects", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        path: workspaceJson.path,
        name: workspaceJson.name ?? "Workspace",
      }),
    });
    const createJson = await createRes.json();
    if (!createRes.ok) {
      throw new Error(createJson.error || `Failed to create workspace (${createRes.status})`);
    }

    return adoptProject(createJson as ProjectSummary);
  }, [loadProjects]);

  const openChat = async (chat: ChatSummary) => {
    if (renamingId) cancelRename();
    setActiveId(chat.id);
    setActiveTitle(chat.title);
    setError(null);
    setLastThought(null);
    try {
      const res = await fetch(`/api/chats/${chat.id}`, { cache: "no-store" });
      const json = await res.json();
      setMessages(json.messages ?? []);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const prefetchDapp = useCallback((projectPath: string, sessionId: string, userText: string) => {
    void fetch("/api/dapp/prefetch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        project_path: projectPath,
        session_id: sessionId,
        user_text: userText,
      }),
    })
      .then(async (res) => res.json())
      .then((json: { ok?: boolean; reused?: boolean; templateTitle?: string }) => {
        if (json.ok && json.reused) {
          setDappRefreshToken((value) => value + 1);
          setDappGenerating(false);
          if (json.templateTitle) {
            setDappLibraryHit(json.templateTitle);
            window.setTimeout(() => setDappLibraryHit(null), 4000);
          }
        }
      })
      .catch(() => {
        /* speculative prefetch is best-effort */
      });
  }, []);

  const stopSubtextObserver = useCallback(() => {
    // Intentionally do NOT abort the observer here. Aborting mid-stream cancels
    // the local model request — which on a cold model (17GB, ~30s to load)
    // means zero tokens render before the answer arrives, and also keeps the
    // model perpetually cold. Instead we let it finish in the background: it
    // settles into `lastThought` when done (see the stream loop), and the model
    // stays warm for the next turn. Here we only hide the live pill and persist
    // whatever partial thought exists so something shows immediately.
    const partial = latentTextRef.current.trim();
    if (partial) {
      setLastThought({ text: partial, words: partial.split(/\s+/).slice(-16) });
      appendThought(partial);
    }
    setLatentWords([]);
    setLatentPhase(null);
  }, []);

  /** Compact a raw narration into a readable thought (sentence-bounded). */
  const trimThought = useCallback((raw: string): string => {
    const collapsed = raw.split(/\s+/).join(" ").trim();
    if (collapsed.length <= 280) return collapsed;
    const head = collapsed.slice(0, 280);
    const cut = Math.max(head.lastIndexOf(". "), head.lastIndexOf("! "), head.lastIndexOf("? "));
    return cut > 60 ? head.slice(0, cut + 1) : `${head.trimEnd()}…`;
  }, []);

  const thoughtStorageKey = (sid: string | null) => `neura-thoughts-${sid ?? "draft"}`;

  const appendThought = useCallback((raw: string) => {
    const text = trimThought(raw);
    if (!text) return;
    setThoughtLog((log) => {
      if (log[0]?.text === text) return log;
      const next = [{ text, at: Date.now() }, ...log].slice(0, 20);
      try {
        sessionStorage.setItem(thoughtStorageKey(activeIdRef.current), JSON.stringify(next));
      } catch { /* storage full/blocked — history stays in-memory */ }
      return next;
    });
  }, [trimThought]);

  useEffect(() => {
    // Chat switched: load that chat's thought history.
    try {
      const raw = sessionStorage.getItem(thoughtStorageKey(activeId));
      setThoughtLog(raw ? JSON.parse(raw) : []);
    } catch {
      setThoughtLog([]);
    }
  }, [activeId]);

  const startSubtextObserver = useCallback((text: string) => {
    const cfg = subtextConfigRef.current;
    if (!cfg || !cfg.enabled || !cfg.endpoint) return;
    // Abort any prior observer without clearing UI (a fresh one follows).
    const prior = subtextAbortRef.current;
    if (prior) {
      try {
        prior.abort();
      } catch {
        /* ignore */
      }
    }
    const controller = new AbortController();
    subtextAbortRef.current = controller;
    latentTextRef.current = "";
    setLastThought(null);
    setLatentWords([]);
    setLatentPhase("thinking");

    const context = messagesRef.current
      .filter((m) => m.text.trim())
      .slice(-6)
      .map((m) => ({ role: m.role, content: m.text }));

    void (async () => {
      try {
        const res = await fetch(cfg.endpoint, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ message: text, context }),
          signal: controller.signal,
        });
        if (!res.body) return;
        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          let sep: number;
          while ((sep = buffer.indexOf("\n\n")) >= 0) {
            const rawEvent = buffer.slice(0, sep);
            buffer = buffer.slice(sep + 2);
            const dataLine = rawEvent
              .split("\n")
              .find((l) => l.startsWith("data:"));
            if (!dataLine) continue;
            let frame: { type?: string; phase?: string; text?: string; words?: string[] };
            try {
              frame = JSON.parse(dataLine.slice(5).trim());
            } catch {
              continue;
            }
            if (frame.type === "frame") {
              if (typeof frame.text === "string" && frame.text.trim()) {
                latentTextRef.current = frame.text;
                setLatentLine(frame.text.trim());
              }
              if (Array.isArray(frame.words) && frame.words.length) {
                setLatentWords(frame.words.slice(-12));
              }
              if (frame.phase) setLatentPhase(frame.phase);
            } else if (frame.type === "done" || frame.type === "error") {
              const finalText = latentTextRef.current.trim();
              if (finalText) {
                setLastThought({ text: finalText, words: finalText.split(/\s+/).slice(-16) });
                appendThought(finalText);
              }
              return;
            }
          }
        }
      } catch {
        /* aborted or network error; observer is best-effort */
      } finally {
        if (subtextAbortRef.current === controller) {
          subtextAbortRef.current = null;
        }
      }
    })();
  }, []);

  const finalizeTurn = useCallback((
    json: ChatTurnResult,
    sessionId: string | null,
    projectPath: string | null,
    originalText: string,
  ) => {
    if (json.session_id) setActiveId(json.session_id);
    if (json.title) setActiveTitle(json.title);
    const reply = json.text ?? "";
    setMessages((m) => {
      const last = m[m.length - 1];
      if (last?.role === "assistant") {
        if (last.text === reply) return m;
        return [...m.slice(0, -1), { role: "assistant", text: reply }];
      }
      return [...m, { role: "assistant", text: reply }];
    });
    void loadProjects();
    void loadChats(activeProjectPathRef.current);
    if (projectPath && reply.trim()) {
      setDappGenerating(true);
    }
    if (projectPath && json.session_id && !sessionId) {
      prefetchDapp(projectPath, json.session_id, originalText);
    }
  }, [loadChats, loadProjects, prefetchDapp]);

  /** Legacy non-streaming turn (fallback when /api/chat/stream is unavailable). */
  const sendViaJson = useCallback(async (
    text: string,
    sessionId: string | null,
    projectPath: string | null,
  ): Promise<string | null> => {
    const res = await fetch("/api/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        session_id: sessionId,
        message: text,
        working_dir: sessionId ? undefined : projectPath ?? undefined,
      }),
    });
    const json = (await res.json()) as ChatTurnResult;
    if (json.error) {
      setError(json.error);
      return sessionId;
    }
    finalizeTurn(json, sessionId, projectPath, text);
    return json.session_id ?? sessionId;
  }, [finalizeTurn]);

  const sendMessage = useCallback(async (
    text: string,
    sessionId: string | null,
    projectPath: string | null,
    showUserMessage: boolean,
  ): Promise<string | null> => {
    if (showUserMessage) {
      setMessages((m) => [...m, { role: "user", text }]);
    }
    startSubtextObserver(text);
    setSending(true);
    setError(null);
    setDappLibraryHit(null);
    setLiveTools([]);
    liveReasoningRef.current = "";
    setLiveReasoning("");
    setLatentLine("");
    setTurnNote(null);
    currentSendRef.current = text;
    sendingSessionRef.current = sessionId;
    if (projectPath && sessionId) {
      prefetchDapp(projectPath, sessionId, text);
    }

    let streamedText = "";
    const updateLiveAssistant = (value: string) => {
      setMessages((m) => {
        const last = m[m.length - 1];
        if (last?.role === "assistant") {
          return [...m.slice(0, -1), { role: "assistant", text: value }];
        }
        return [...m, { role: "assistant", text: value }];
      });
    };
    const upsertTool = (id: string, name: string, status: "running" | "done" | "error") => {
      setLiveTools((tools) => {
        const idx = tools.findIndex((t) => t.id === id);
        if (idx >= 0) {
          const next = tools.slice();
          next[idx] = { id, name, status };
          return next;
        }
        return [...tools, { id, name, status }].slice(-10);
      });
    };

    try {
      const res = await fetch("/api/chat/stream", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          session_id: sessionId,
          message: text,
          working_dir: sessionId ? undefined : projectPath ?? undefined,
        }),
      });
      if (!res.ok || !res.body) {
        // Older server without the stream endpoint — fall back.
        return await sendViaJson(text, sessionId, projectPath);
      }
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      let resultSid: string | null = sessionId;
      let sawDone = false;
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let sep: number;
        while ((sep = buffer.indexOf("\n\n")) >= 0) {
          const rawEvent = buffer.slice(0, sep);
          buffer = buffer.slice(sep + 2);
          const dataLine = rawEvent.split("\n").find((l) => l.startsWith("data:"));
          if (!dataLine) continue;
          let event: Record<string, unknown>;
          try {
            event = JSON.parse(dataLine.slice(5).trim());
          } catch {
            continue;
          }
          const kind = event.type as string | undefined;
          switch (kind) {
            case "start":
            case "session":
              if (typeof event.session_id === "string") {
                resultSid = event.session_id;
                sendingSessionRef.current = event.session_id;
              }
              break;
            case "busy":
              // A turn is already running for this session: keep the message
              // in the queue rather than double-running it.
              setTurnNote("previous turn still running — message queued");
              if (!pendingSendRef.current.includes(text)) {
                pendingSendRef.current.unshift(text);
                setQueuedCount(pendingSendRef.current.length);
              }
              break;
            case "queued":
              setTurnNote(String(event.note ?? "queued behind other turns…"));
              break;
            case "status_detail":
              setTurnNote(String(event.detail ?? ""));
              break;
            case "text_delta":
              streamedText += String(event.text ?? "");
              updateLiveAssistant(streamedText);
              break;
            case "text_replace":
              streamedText = String(event.text ?? "");
              updateLiveAssistant(streamedText);
              break;
            case "reasoning_delta": {
              liveReasoningRef.current += String(event.text ?? "");
              const tail = liveReasoningRef.current;
              setLiveReasoning(tail.length > 900 ? `…${tail.slice(-900)}` : tail);
              break;
            }
            case "tool_start":
            case "tool_exec":
              upsertTool(String(event.id ?? event.name ?? "tool"), String(event.name ?? "tool"), "running");
              break;
            case "tool_done":
              upsertTool(
                String(event.id ?? event.name ?? "tool"),
                String(event.name ?? "tool"),
                event.error ? "error" : "done",
              );
              break;
            case "memory_injected":
              upsertTool("memory", `memory ×${String(event.count ?? "?")}`, "done");
              break;
            case "subtext_latent": {
              // Real observer frames (J-space / logit-lens / OSS / stage).
              const phase = typeof event.phase === "string" ? event.phase : null;
              if (phase) setLatentPhase(phase);
              const latent = Array.isArray(event.latent) ? (event.latent as string[]) : [];
              if (latent.length > 0) setLatentWords(latent.slice(-12));
              if (typeof event.text === "string" && event.text.trim()) {
                latentTextRef.current = event.text;
                setLatentLine(event.text.trim());
                // The observer's final polished thought lands in the history.
                if (phase === "oss:thought") appendThought(event.text);
              }
              break;
            }
            case "done": {
              sawDone = true;
              const json = event as unknown as ChatTurnResult;
              finalizeTurn(json, sessionId, projectPath, text);
              resultSid = json.session_id ?? resultSid;
              break;
            }
            case "error":
              sawDone = true;
              setError(String(event.message ?? "turn failed"));
              break;
            default:
              break;
          }
          if (sawDone) break;
        }
        if (sawDone) {
          // The turn is over: stop reading immediately so the composer
          // unlocks even if the transport lingers before closing.
          try {
            await reader.cancel();
          } catch {
            /* already closed */
          }
          break;
        }
      }
      if (!sawDone && streamedText) {
        // Connection ended early: keep what streamed rather than losing it.
        updateLiveAssistant(streamedText);
      }
      return resultSid;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return sessionId;
    } finally {
      setSending(false);
      stopSubtextObserver();
      setLiveTools([]);
      currentSendRef.current = null;
    }
  }, [finalizeTurn, prefetchDapp, sendViaJson, startSubtextObserver, stopSubtextObserver]);

  const pumpSendQueue = useCallback(async () => {
    if (pumpQueueRef.current) return;
    pumpQueueRef.current = true;
    try {
      let sessionId = activeIdRef.current;
      let projectPath = activeProjectPathRef.current;
      if (!projectPath) {
        projectPath = await ensureActiveProject();
      }
      while (pendingSendRef.current.length > 0) {
        const next = pendingSendRef.current.shift()!;
        setQueuedCount(pendingSendRef.current.length);
        sessionId = await sendMessage(next, sessionId, projectPath, false);
      }
    } finally {
      pumpQueueRef.current = false;
    }
  }, [ensureActiveProject, sendMessage]);

  const send = useCallback(async () => {
    const textarea = inputRef.current;
    const text = textarea?.value.trim() ?? "";
    if (!text) return;

    if (textarea) textarea.value = "";
    finalTranscriptRef.current = "";

    let projectPath = activeProjectPathRef.current;
    if (!projectPath) {
      try {
        projectPath = await ensureActiveProject();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        if (textarea) textarea.value = text;
        return;
      }
    }

    if (sending) {
      // Re-pressing send with the same text while a turn runs used to queue
      // duplicate turns (each re-asking the model). Ignore exact duplicates
      // of the in-flight or already-queued message.
      if (text === currentSendRef.current || pendingSendRef.current.includes(text)) {
        return;
      }
      pendingSendRef.current.push(text);
      setQueuedCount(pendingSendRef.current.length);
      setMessages((m) => [...m, { role: "user", text }]);
      return;
    }

    await sendMessage(text, activeId, projectPath, true);
    await pumpSendQueue();
  }, [activeId, ensureActiveProject, pumpSendQueue, sendMessage, sending]);

  useEffect(() => {
    if (sending || pendingSendRef.current.length === 0) return;
    void pumpSendQueue();
  }, [sending, pumpSendQueue]);

  const cancelTurn = useCallback(async () => {
    const sid = sendingSessionRef.current ?? activeIdRef.current;
    if (!sid) return;
    setTurnNote("stopping…");
    try {
      await fetch("/api/chat/cancel", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ session_id: sid }),
      });
    } catch {
      /* best-effort; the stream also cancels when the reader closes */
    }
  }, []);

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
      await fetch("/api/shutdown", { method: "POST" });
    } catch { /* expected — the server is going away */ }
  };

  const copyMessage = useCallback(async (messageKey: string, text: string) => {
    if (!text.trim()) return;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const textarea = document.createElement("textarea");
      textarea.value = text;
      textarea.style.position = "fixed";
      textarea.style.opacity = "0";
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand("copy");
      document.body.removeChild(textarea);
    }
    setCopiedMessageKey(messageKey);
    if (copyResetTimerRef.current !== null) {
      window.clearTimeout(copyResetTimerRef.current);
    }
    copyResetTimerRef.current = window.setTimeout(() => {
      setCopiedMessageKey(null);
      copyResetTimerRef.current = null;
    }, 1200);
  }, []);

  const handleCopyHover = useCallback((messageKey: string, text: string) => {
    if (copyHoverLockRef.current === messageKey) return;
    copyHoverLockRef.current = messageKey;
    void copyMessage(messageKey, text);
  }, [copyMessage]);

  const handleCopyHoverEnd = useCallback((messageKey: string) => {
    if (copyHoverLockRef.current === messageKey) {
      copyHoverLockRef.current = null;
    }
  }, []);

  const lastMessage = messages[messages.length - 1];
  const streamingAssistant =
    sending && lastMessage?.role === "assistant" && Boolean(lastMessage.text);
  const empty = messages.length === 0 && !sending;
  const inProject = Boolean(activeProjectPath);
  const dappEnabled = inProject && Boolean(activeProjectId);

  const workspace = (
    <>
      <aside
        className="sidebar sidebar--chats sidebar--dapp"
        aria-label="Dapp files"
        aria-hidden={!inProject}
      >
        <div className="sidebar__head sidebar__head--dapp">
          <div className="sidebar__chat-row">
            <button
              type="button"
              className={`sidebar__mode-btn${sidebarMode === "chats" ? " is-active" : ""}`}
              onClick={() => setSidebarMode((mode) => (mode === "chats" ? "dapp" : "chats"))}
            >
              {sidebarMode === "chats" ? "View code" : "View chats"}
            </button>
            <button type="button" className="sidebar__action" onClick={toggleChatHidden}>
              {chatHidden ? "Show chat" : "Hide chat"}
            </button>
            <button type="button" className="sidebar__action" onClick={newChat}>
              + New
            </button>
          </div>
        </div>
        <div className={`sidebar__scroll${sidebarMode === "dapp" ? " sidebar__scroll--dapp" : ""}`}>
          {sidebarMode === "dapp" && dappEnabled ? (
            <DappSidebar />
          ) : (
            <>
              {chats.length === 0 ? (
                <p className="sidebar__empty">
                  No chats in this project.<br />
                  Send a message to start one.
                </p>
              ) : (
                chats.map((c) => (
                  <div key={c.id}>
                    {renamingId === c.id ? (
                      <div className="sidebar__rename">
                        <input
                          ref={renameInputRef}
                          className="sidebar__rename-input"
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
                        type="button"
                        className={`sidebar__item ${c.id === activeId ? "is-active" : ""}`}
                        onClick={() => void openChat(c)}
                        onDoubleClick={(e) => {
                          e.preventDefault();
                          startRename(c);
                        }}
                      >
                        <span className="sidebar__item-col">
                          <span className="sidebar__item-title" title="Double-click to rename">
                            {c.title}
                          </span>
                          <span className="sidebar__item-sub">
                            {c.messageCount} msg{c.messageCount === 1 ? "" : "s"}
                            {c.model && c.model !== "unknown" ? ` · ${c.model}` : ""}
                            {c.titleLocked ? " · pinned" : ""}
                          </span>
                        </span>
                      </button>
                    )}
                  </div>
                ))
              )}
            </>
          )}
        </div>
      </aside>

      <main
        className={[
          "main",
          empty ? "main--empty" : "",
          inProject ? "main--dapp-open" : "",
          chatHidden ? "main--chat-hidden" : "",
        ].filter(Boolean).join(" ")}
      >
        {dappEnabled && <DappPreview />}
        <div className={`main__chat${chatHidden ? " main__chat--hidden" : ""}`}>
        <header className="head">
          <div className="head__left">
            <h1 className={`brand ${sending ? "brand--busy" : ""}`}>NEURA</h1>
          </div>
          <div className="head__right">
            <button type="button" className="icon-btn" title="Cognition — knowledge graph, predictions, sleep" onClick={() => setShowCognition((v) => !v)}>
              <CognitionIcon />
            </button>
            <button type="button" className="icon-btn" title="Live state" onClick={() => setShowInfo((v) => !v)}>ⓘ</button>
            <button type="button" className="icon-btn icon-btn--power" title="Shut down neura" onClick={() => setShutState("confirm")}>
              <PowerIcon />
            </button>
          </div>
        </header>

        <div className="log" ref={scrollRef}>
          {messages.map((m, i) => {
            const isStreamingAssistant =
              sending && i === messages.length - 1 && m.role === "assistant" && Boolean(m.text);
            const messageKey = `${m.role}-${i}`;
            const copied = copiedMessageKey === messageKey;
            const canCopy = Boolean(m.text.trim());
            const isUser = m.role === "user";
            const copyButton = canCopy ? (
              <button
                type="button"
                className={[
                  "msg__copy",
                  isUser ? "msg__copy--outside" : "",
                  copied ? "msg__copy--copied" : "",
                ].filter(Boolean).join(" ")}
                title={copied ? "Copied" : "Hover to copy"}
                aria-label={copied ? "Copied" : "Hover to copy message"}
                onMouseEnter={() => handleCopyHover(messageKey, m.text)}
                onMouseLeave={() => handleCopyHoverEnd(messageKey)}
                onFocus={() => handleCopyHover(messageKey, m.text)}
                onBlur={() => handleCopyHoverEnd(messageKey)}
              >
                {copied ? <CheckIcon /> : <CopyIcon />}
              </button>
            ) : null;
            return (
              <div key={i} className={`msg msg--${m.role}`}>
                {m.role === "assistant" && <span className="msg__who">{activeTitle}</span>}
                <div className={`msg__shell${isUser ? " msg__shell--user" : ""}`}>
                  <div
                    className={[
                      "msg__body",
                      isUser ? "msg__body--user" : "",
                      isStreamingAssistant ? "msg__body--streaming" : "",
                      copied ? "msg__body--copied" : "",
                    ].filter(Boolean).join(" ")}
                  >
                    {m.tools && m.tools.length > 0 && <div className="msg__tools">{m.tools.join(", ")}</div>}
                    <MarkdownMessage text={m.text} active={isStreamingAssistant} />
                    {!isUser && copyButton}
                  </div>
                  {isUser && copyButton}
                </div>
              </div>
            );
          })}
          {sending && !streamingAssistant && (
            <div className="msg msg--assistant">
              <span className="msg__who">{activeTitle}</span>
              <div className="msg__body msg__body--thinking">{turnNote || "thinking…"}</div>
            </div>
          )}
          {sending && liveTools.length > 0 && (
            <div className="livetools" title="Tools running in this turn">
              {liveTools.map((t) => (
                <span key={t.id} className={`livetools__chip livetools__chip--${t.status}`}>
                  {t.status === "running" ? "▸ " : t.status === "error" ? "✕ " : "✓ "}
                  {t.name}
                </span>
              ))}
            </div>
          )}
          {sending && liveReasoning && (
            <details className="reasoning" open>
              <summary className="reasoning__summary">💭 model reasoning (live — not part of the answer)</summary>
              <div className="reasoning__body">{liveReasoning}</div>
            </details>
          )}
          {sending && (latentLine || latentWords.length > 0 || latentPhase) && (
            <div className="subtext" title="Live latent thoughts from the local Subtext observer">
              <span className="subtext__label">
                💭 {latentPhase ? latentPhase.replace(/^(stage|oss|companion|subtext):/, "") : "latent"}
              </span>
              {latentLine ? (
                // A readable narration sentence beats a strip of word salad.
                <span className="subtext__line">{latentLine}</span>
              ) : (
                <span className="subtext__words">
                  {latentWords.length > 0
                    ? latentWords.map((w, i) => (
                        <span key={`${w}-${i}`} className="subtext__word">
                          {w}
                        </span>
                      ))
                    : <span className="subtext__word subtext__word--muted">listening…</span>}
                </span>
              )}
              {latentLine && latentWords.length > 0 && (
                <span className="subtext__words subtext__words--secondary">
                  {latentWords.slice(-6).map((w, i) => (
                    <span key={`${w}-${i}`} className="subtext__word">{w}</span>
                  ))}
                </span>
              )}
            </div>
          )}
          {!sending && thoughtLog.length > 0 && (
            <details className="thought">
              <summary className="thought__summary">
                💭 thoughts ({thoughtLog.length}) — {trimThought(thoughtLog[0].text).slice(0, 72)}
              </summary>
              <div className="thought__body">
                {thoughtLog.map((t, i) => (
                  <div key={t.at + i} className="thought__entry">
                    <span className="thought__time">
                      {new Date(t.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                    </span>
                    <span>{t.text}</span>
                  </div>
                ))}
              </div>
            </details>
          )}
        </div>

        {empty && (
          <div className="hero">
            <span className={`brand brand--hero ${sending ? "brand--busy" : ""}`}>NEURA</span>
            {activeProjectPath ? (
              <>
                <p className="hero__project">{activeProjectName}</p>
                <p className="hero__sub">
                  New chat in this project. Messages run neura with this folder as the working directory.
                </p>
              </>
            ) : (
              <p className="hero__sub">
                Message neura to start. A workspace project is created automatically on your first chat.
              </p>
            )}
          </div>
        )}

        {error && <div className="error-banner">{error}</div>}

        <div className="composer-wrap">
          <div
            className={[
              "composer",
              sending ? "composer--busy" : "",
              voiceActive ? "composer--voice" : "",
            ].filter(Boolean).join(" ")}
          >
            {voiceSupported && voiceActive && voiceStatus && (
              <p className="voice-hint voice-hint--banner">{voiceStatus}</p>
            )}
            {!voiceSupported && voiceStatus && (
              <p className="voice-hint voice-hint--error">{voiceStatus}</p>
            )}
            {queuedCount > 0 && (
              <p className="queue-hint">{queuedCount} message{queuedCount === 1 ? "" : "s"} queued</p>
            )}
            <div className="composer__field">
              <textarea
                ref={inputRef}
                placeholder={
                  sending
                    ? "Keep talking or typing your next message…"
                    : voiceSupported
                      ? "Message neura… (hold anywhere to dictate)"
                      : "Message neura…"
                }
                disabled={false}
                onFocus={markTyping}
                onInput={markTyping}
                onKeyDown={onKey}
                rows={1}
              />
              {sending && (
                <button
                  type="button"
                  className="send-btn send-btn--stop"
                  onClick={() => void cancelTurn()}
                  title="Stop generating"
                >
                  <StopIcon />
                </button>
              )}
              <button
                type="button"
                className="send-btn"
                onClick={() => void send()}
                disabled={sending}
                title={sending ? "Queue message" : "Send"}
              >
                <SendIcon />
              </button>
            </div>
          </div>
        </div>
        </div>
      </main>
    </>
  );

  return (
    <div className={`app ${inProject ? "app--in-project" : ""} ${voiceActive ? "app--voice-active" : ""} ${voiceHoldArmed ? "app--voice-hold" : ""}`}>
      <aside
        className={`sidebar sidebar--projects ${inProject ? "sidebar--collapsed" : ""}`}
        aria-label="Projects"
      >
        <div className="sidebar__head">
          {inProject ? (
            <button
              type="button"
              className="sidebar__rail-btn"
              onClick={exitProject}
              title="All projects"
            >
              ←
            </button>
          ) : (
            <span className="sidebar__label">Projects</span>
          )}
          <button
            type="button"
            className="sidebar__action"
            onClick={openAddProject}
            title="Add project"
          >
            {inProject ? "+" : "+ Add"}
          </button>
        </div>
        <div className="sidebar__scroll">
          {projects.length === 0 ? (
            <p className="sidebar__empty">
              No projects yet.<br />
              Add a folder to start chatting.
            </p>
          ) : (
            projects.map((project) => (
              <button
                key={project.id}
                type="button"
                className={`sidebar__item ${project.path === activeProjectPath ? "is-active" : ""}`}
                onClick={() => selectProject(project)}
                title={project.path}
              >
                {inProject ? (
                  <span className="sidebar__badge">{projectBadge(project.name)}</span>
                ) : (
                  <>
                    <span className="sidebar__item-col">
                      <span className="sidebar__item-title">{project.name}</span>
                      <span className="sidebar__item-sub">{project.path}</span>
                    </span>
                    <span className="sidebar__item-meta">{project.chatCount}</span>
                  </>
                )}
              </button>
            ))
          )}
        </div>
      </aside>

      {dappEnabled ? (
        <DappProvider
          projectId={activeProjectId!}
          projectPath={activeProjectPath!}
          sessionId={activeId}
          refreshToken={dappRefreshToken}
          generating={dappGenerating}
          libraryHit={dappLibraryHit}
          interactive={chatHidden}
        >
          {workspace}
        </DappProvider>
      ) : (
        workspace
      )}

      {showAddProject && (
        <div className="modal-scrim" onClick={closeAddProject}>
          <div className="modal modal--form" onClick={(e) => e.stopPropagation()}>
            <h3>Add project</h3>
            <p>
              New projects default to <code>~/.neura/projects/</code>. Change the path to use any folder on disk.
              Chats run neura with that directory as the working folder.
            </p>
            <label htmlFor="project-name">Display name (optional)</label>
            <input
              id="project-name"
              value={projectNameDraft}
              onChange={(e) => setProjectNameDraft(e.target.value)}
              placeholder="my-repo"
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void submitAddProject();
                }
              }}
            />
            <label htmlFor="project-path">Path</label>
            <div className="modal__path-row">
              <input
                id="project-path"
                ref={projectPathInputRef}
                value={projectPathDraft}
                onChange={(e) => {
                  projectPathEditedRef.current = true;
                  setProjectPathDraft(e.target.value);
                }}
                placeholder="/home/you/.neura/projects/my-repo"
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void submitAddProject();
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    closeAddProject();
                  }
                }}
              />
              <button
                type="button"
                className="btn modal__browse-btn"
                onClick={() => setShowFolderPicker(true)}
              >
                Browse…
              </button>
            </div>
            <div className="modal__actions">
              <button type="button" className="btn" onClick={closeAddProject}>Cancel</button>
              <button
                type="button"
                className="btn btn--primary"
                onClick={() => void submitAddProject()}
                disabled={addingProject}
              >
                {addingProject ? "Adding…" : "Add project"}
              </button>
            </div>
          </div>
        </div>
      )}

      {showAddProject && showFolderPicker && (
        <FolderPickerModal
          initialPath={projectPathDraft}
          onSelect={(path) => {
            projectPathEditedRef.current = true;
            setProjectPathDraft(path);
            setShowFolderPicker(false);
            window.setTimeout(() => projectPathInputRef.current?.focus(), 0);
          }}
          onClose={() => setShowFolderPicker(false)}
        />
      )}

      {shutState === "confirm" && (
        <div className="modal-scrim" onClick={() => setShutState("idle")}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal__icon"><PowerIcon /></div>
            <h3>Shut down neura?</h3>
            <p>This kills every neura process — the agent, this web UI, and any running sessions. Run <code>neura</code> in a terminal to start again.</p>
            <div className="modal__actions modal__actions--center">
              <button type="button" className="btn" onClick={() => setShutState("idle")}>Cancel</button>
              <button type="button" className="btn btn--danger" onClick={shutdown}>Shut down</button>
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

      {showCognition && (
        <CognitionPanel projectPath={activeProjectPath} onClose={() => setShowCognition(false)} />
      )}

      {showInfo && state && (
        <aside className="info-drawer" onClick={() => setShowInfo(false)}>
          <div className="info-card" onClick={(e) => e.stopPropagation()}>
            <div className="info-card__head">
              <h3>Live state</h3>
              <button type="button" className="icon-btn" onClick={() => setShowInfo(false)}>✕</button>
            </div>
            <dl className="info-grid">
              <div><dt>server</dt><dd>{state.serverName ?? serverName}</dd></div>
              <div><dt>git branch</dt><dd>{state.git?.branch}</dd></div>
              <div><dt>uncommitted</dt><dd>{state.git?.status?.length ?? 0}</dd></div>
              <div><dt>rust files</dt><dd>{state.repo?.rustFiles}</dd></div>
              <div><dt>python files</dt><dd>{state.repo?.pythonFiles}</dd></div>
              <div><dt>ts files</dt><dd>{state.repo?.tsFiles}</dd></div>
              <div><dt>server pid</dt><dd>{state.runtime?.pid}</dd></div>
              <div><dt>projects</dt><dd>{projects.length}</dd></div>
              <div><dt>chats</dt><dd>{chats.length}</dd></div>
            </dl>
          </div>
        </aside>
      )}
    </div>
  );
}

export default App;
