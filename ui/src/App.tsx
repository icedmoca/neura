import { useEffect, useMemo, useState } from "react";
import "./index.css";

type MemoryNode = {
  id: string;
  label: string;
  layer: "ctx" | "episodic" | "semantic" | "procedural" | "working" | "artifact";
  tokens: number;
  trust: number;
  heat: number;
  links: string[];
};

type ToolEvent = {
  tool: string;
  purpose: string;
  status: "live" | "verified" | "queued" | "risk";
  ms: number;
};

type KcodeState = {
  generatedAt: number;
  root: string;
  kcodeHome: string;
  git: { branch: string; status: string[]; commits: string[]; remotes: string[] };
  repo: { rustFiles: number; pythonFiles: number; tsFiles: number };
  runtime: { pid: number; cwd: string; activeMarkers: string[]; logs: { name: string; size: number; mtime: number }[]; eventTail: unknown[] };
  memory: { ctxBands: { name: string; used: number; source: string }[]; layers: string[] };
};

const fallbackNodes: MemoryNode[] = [
  { id: "sys", label: "System + developer contract", layer: "ctx", tokens: 1680, trust: 0.98, heat: 0.88, links: ["policy", "todo", "tools"] },
  { id: "user", label: "Current user intent", layer: "working", tokens: 360, trust: 0.94, heat: 1, links: ["ui", "memory", "self"] },
  { id: "ui", label: "Kcode cockpit UI implementation", layer: "artifact", tokens: 820, trust: 0.86, heat: 0.91, links: ["neura", "tests"] },
  { id: "neura", label: "Neura scaffold/assets copied", layer: "episodic", tokens: 540, trust: 0.82, heat: 0.73, links: ["ui", "style"] },
  { id: "memory", label: "Memory graph + ctx visualizer", layer: "semantic", tokens: 940, trust: 0.9, heat: 0.97, links: ["sys", "tools", "tests"] },
  { id: "tools", label: "Tool calls, traces, diffs", layer: "procedural", tokens: 710, trust: 0.92, heat: 0.81, links: ["tests", "self"] },
  { id: "tests", label: "Build/test/commit gates", layer: "procedural", tokens: 430, trust: 0.96, heat: 0.64, links: ["ui", "self"] },
  { id: "self", label: "Self improvement backlog", layer: "semantic", tokens: 680, trust: 0.88, heat: 0.79, links: ["memory", "tests"] },
];

const toolEvents: ToolEvent[] = [
  { tool: "agentgrep", purpose: "semantic code search and trace discovery", status: "verified", ms: 38 },
  { tool: "bash", purpose: "repo inspection, builds, git hygiene, server launch", status: "live", ms: 71 },
  { tool: "read/write/edit", purpose: "source modification with inspectable diffs", status: "verified", ms: 4 },
  { tool: "kcode-ui-server", purpose: "local UI hosting and /api/state bridge", status: "live", ms: 2 },
  { tool: "browser/mouse", purpose: "interactive validation and screenshots", status: "queued", ms: 0 },
];

const panels = ["Mission", "Memory", "Tools", "Runtime", "Self-Evolution"] as const;
type Panel = (typeof panels)[number];

function pct(value: number) {
  return `${Math.round(value * 100)}%`;
}

function useKcodeState() {
  const [state, setState] = useState<KcodeState | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const res = await fetch("/api/state", { cache: "no-store" });
        if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
        const json = (await res.json()) as KcodeState;
        if (alive) { setState(json); setError(null); }
      } catch (err) {
        if (alive) setError(err instanceof Error ? err.message : String(err));
      }
    };
    load();
    const timer = window.setInterval(load, 3500);
    return () => { alive = false; window.clearInterval(timer); };
  }, []);
  return { state, error };
}

function buildNodes(state: KcodeState | null): MemoryNode[] {
  if (!state) return fallbackNodes;
  return [
    { id: "git", label: `git:${state.git.branch}`, layer: "artifact", tokens: state.git.status.length * 80 + 240, trust: 0.93, heat: state.git.status.length ? 0.94 : 0.55, links: ["repo", "tests"] },
    { id: "repo", label: `${state.repo.rustFiles} Rust · ${state.repo.pythonFiles} Python · ${state.repo.tsFiles} TS`, layer: "semantic", tokens: 980, trust: 0.9, heat: 0.82, links: ["git", "tools", "ui"] },
    { id: "runtime", label: `server pid ${state.runtime.pid}`, layer: "working", tokens: 420, trust: 0.92, heat: 0.88, links: ["events", "ctx"] },
    { id: "events", label: `${state.runtime.eventTail.length} recent events`, layer: "episodic", tokens: state.runtime.eventTail.length * 40, trust: 0.84, heat: Math.min(1, state.runtime.eventTail.length / 30), links: ["runtime", "memory"] },
    { id: "ctx", label: "live context pressure", layer: "ctx", tokens: 760, trust: 0.88, heat: 0.9, links: ["memory", "repo"] },
    { id: "memory", label: state.memory.layers.join(" + "), layer: "semantic", tokens: 940, trust: 0.9, heat: 0.97, links: ["ctx", "tools"] },
    { id: "tools", label: "tool fabric", layer: "procedural", tokens: 650, trust: 0.91, heat: 0.79, links: ["tests", "runtime"] },
    { id: "ui", label: "Kcode UI cockpit", layer: "artifact", tokens: 850, trust: 0.87, heat: 0.93, links: ["repo", "memory"] },
    { id: "tests", label: "build and commit gates", layer: "procedural", tokens: 410, trust: 0.95, heat: 0.66, links: ["git", "tools"] },
  ];
}

function MemoryConstellation({ state }: { state: KcodeState | null }) {
  const nodes = useMemo(() => buildNodes(state), [state]);
  return (
    <section className="glass memory-card">
      <div className="section-heading">
        <div><p className="eyebrow">advanced memory</p><h2>Context constellation</h2></div>
        <span className="pill hot">live ctx aware</span>
      </div>
      <div className="constellation" aria-label="Kcode memory graph visualization">
        <svg viewBox="0 0 760 420" role="img">
          <defs><radialGradient id="nodeGlow"><stop offset="0%" stopColor="#dff7ff"/><stop offset="45%" stopColor="#7dd3fc"/><stop offset="100%" stopColor="#312e81"/></radialGradient></defs>
          {nodes.flatMap((node, i) => node.links.map((link) => {
            const j = nodes.findIndex((n) => n.id === link);
            if (j < 0) return null;
            const a = polar(i, nodes.length); const b = polar(j, nodes.length);
            return <line key={`${node.id}-${link}`} x1={a.x} y1={a.y} x2={b.x} y2={b.y} className="edge" />;
          }))}
          {nodes.map((node, i) => {
            const p = polar(i, nodes.length);
            return <g key={node.id} className={`node node-${node.layer}`}><circle cx={p.x} cy={p.y} r={22 + node.heat * 14} /><text x={p.x} y={p.y + 4}>{node.id}</text></g>;
          })}
        </svg>
        <div className="node-list">
          {nodes.map((node) => <article key={node.id}><b>{node.label}</b><span>{node.layer} · {node.tokens} tok · trust {pct(node.trust)}</span><meter min="0" max="1" value={node.heat} /></article>)}
        </div>
      </div>
    </section>
  );
}

function polar(i: number, total: number) {
  const angle = (Math.PI * 2 * i) / total - Math.PI / 2;
  return { x: 380 + Math.cos(angle) * 280, y: 210 + Math.sin(angle) * 150 };
}

function App() {
  const [panel, setPanel] = useState<Panel>("Memory");
  const { state, error } = useKcodeState();
  const ctxBands = state?.memory.ctxBands ?? [
    { name: "Instruction stack", used: 19, source: "system/developer/user" },
    { name: "User goal", used: 12, source: "current prompt" },
    { name: "Repo evidence", used: 27, source: "source scans" },
    { name: "Working plan", used: 17, source: "active implementation" },
    { name: "Generated code", used: 25, source: "ui artifacts" },
  ];

  return (
    <main className="app-shell">
      <aside className="sidebar glass">
        <div className="brand"><span>K</span><div><b>Kcode</b><small>self-evolving cockpit</small></div></div>
        <nav>{panels.map((p) => <button key={p} className={panel === p ? "active" : ""} onClick={() => setPanel(p)}>{p}</button>)}</nav>
        <div className="status-stack">
          <span><i className={state ? "ok" : "warn"}/> {state ? "live api connected" : "static fallback"}</span>
          <span><i className="ok"/> tool fabric online</span>
          <span><i className={error ? "warn" : "ok"}/> {error ? `api: ${error}` : "state refreshing"}</span>
        </div>
      </aside>

      <section className="hero glass">
        <p className="eyebrow">Kcode native UI · {panel}</p>
        <h1>Agent operations, memory, context, tools, runtime state, and self-improvement in one visual surface.</h1>
        <div className="hero-grid">
          <div><b>{state?.repo.rustFiles ?? 0}</b><span>Rust files</span></div>
          <div><b>{state?.repo.pythonFiles ?? 0}</b><span>Python files</span></div>
          <div><b>{state?.runtime.eventTail.length ?? "fallback"}</b><span>recent events</span></div>
          <div><b>{state?.git.branch ?? "local"}</b><span>git branch</span></div>
        </div>
      </section>

      <section className="content-grid">
        <MemoryConstellation state={state} />

        <section className="glass">
          <div className="section-heading"><div><p className="eyebrow">ctx budget</p><h2>Token pressure map</h2></div><span className="pill">summarize before overflow</span></div>
          <div className="ctx-bars">{ctxBands.map((band, i) => <div key={band.name} className="ctx-row"><span title={band.source}>{band.name}</span><div><i className={["violet", "cyan", "green", "amber", "rose"][i % 5]} style={{ width: `${band.used}%` }}/></div><b>{band.used}%</b></div>)}</div>
        </section>

        <section className="glass">
          <div className="section-heading"><div><p className="eyebrow">tool fabric</p><h2>Execution lanes</h2></div><span className="pill hot">auditable</span></div>
          <div className="tool-list">{toolEvents.map((event) => <article key={event.tool} className={event.status}><b>{event.tool}</b><span>{event.purpose}</span><em>{event.status}{event.ms ? ` · ${event.ms}ms` : ""}</em></article>)}</div>
        </section>

        <section className="glass wide">
          <div className="section-heading"><div><p className="eyebrow">runtime</p><h2>Live Kcode state</h2></div><span className="pill">/api/state</span></div>
          <div className="runtime-grid">
            <pre>{(state?.git.status ?? ["API not connected yet"]).join("\n")}</pre>
            <pre>{(state?.git.commits ?? ["Start scripts/kcode-ui-server.py to enable live state"]).join("\n")}</pre>
            <pre>{JSON.stringify(state?.runtime.logs ?? [], null, 2)}</pre>
          </div>
        </section>

        <section className="glass wide">
          <div className="section-heading"><div><p className="eyebrow">self evolution</p><h2>Improvement loop</h2></div><span className="pill">plan → patch → test → commit → push</span></div>
          <div className="loop">{["Observe repo and user intent", "Serve UI and bridge live state", "Visualize ctx, traces, artifacts, risk", "Run Vite/TypeScript build gates", "Commit and push to icedmoca/kcode"].map((step, i) => <div key={step}><b>{String(i + 1).padStart(2, "0")}</b><span>{step}</span></div>)}</div>
        </section>
      </section>
    </main>
  );
}

export default App;
