import { useMemo, useState } from "react";
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

const memoryNodes: MemoryNode[] = [
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
  { tool: "bash", purpose: "repo inspection, builds, git hygiene", status: "live", ms: 71 },
  { tool: "read/write/edit", purpose: "source modification with inspectable diffs", status: "verified", ms: 4 },
  { tool: "browser/mouse", purpose: "interactive UI validation and screenshots", status: "queued", ms: 0 },
  { tool: "schedule", purpose: "follow-up checks for long running jobs", status: "queued", ms: 0 },
];

const ctxBands = [
  { name: "Instruction stack", used: 19, tone: "violet" },
  { name: "User goal", used: 12, tone: "cyan" },
  { name: "Repo evidence", used: 27, tone: "green" },
  { name: "Working plan", used: 17, tone: "amber" },
  { name: "Generated code", used: 25, tone: "rose" },
];

const panels = ["Mission", "Memory", "Tools", "Runtime", "Self-Evolution"] as const;
type Panel = (typeof panels)[number];

function pct(value: number) {
  return `${Math.round(value * 100)}%`;
}

function MemoryConstellation() {
  const nodes = useMemo(() => memoryNodes, []);
  return (
    <section className="glass memory-card">
      <div className="section-heading">
        <div>
          <p className="eyebrow">advanced memory</p>
          <h2>Context constellation</h2>
        </div>
        <span className="pill hot">live ctx aware</span>
      </div>
      <div className="constellation" aria-label="Kcode memory graph visualization">
        <svg viewBox="0 0 760 420" role="img">
          <defs>
            <radialGradient id="nodeGlow"><stop offset="0%" stopColor="#dff7ff"/><stop offset="45%" stopColor="#7dd3fc"/><stop offset="100%" stopColor="#312e81"/></radialGradient>
          </defs>
          {nodes.flatMap((node, i) => node.links.map((link) => {
            const j = nodes.findIndex((n) => n.id === link);
            if (j < 0) return null;
            const a = polar(i, nodes.length);
            const b = polar(j, nodes.length);
            return <line key={`${node.id}-${link}`} x1={a.x} y1={a.y} x2={b.x} y2={b.y} className="edge" />;
          }))}
          {nodes.map((node, i) => {
            const p = polar(i, nodes.length);
            return (
              <g key={node.id} className={`node node-${node.layer}`}>
                <circle cx={p.x} cy={p.y} r={22 + node.heat * 14} />
                <text x={p.x} y={p.y + 4}>{node.id}</text>
              </g>
            );
          })}
        </svg>
        <div className="node-list">
          {nodes.map((node) => (
            <article key={node.id}>
              <b>{node.label}</b>
              <span>{node.layer} · {node.tokens} tok · trust {pct(node.trust)}</span>
              <meter min="0" max="1" value={node.heat} />
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}

function polar(i: number, total: number) {
  const angle = (Math.PI * 2 * i) / total - Math.PI / 2;
  const rx = 280;
  const ry = 150;
  return { x: 380 + Math.cos(angle) * rx, y: 210 + Math.sin(angle) * ry };
}

function App() {
  const [panel, setPanel] = useState<Panel>("Memory");

  return (
    <main className="app-shell">
      <aside className="sidebar glass">
        <div className="brand"><span>K</span><div><b>Kcode</b><small>self-evolving cockpit</small></div></div>
        <nav>{panels.map((p) => <button key={p} className={panel === p ? "active" : ""} onClick={() => setPanel(p)}>{p}</button>)}</nav>
        <div className="status-stack">
          <span><i className="ok"/> repo aware</span>
          <span><i className="ok"/> tool fabric online</span>
          <span><i className="warn"/> commit gate pending</span>
        </div>
      </aside>

      <section className="hero glass">
        <p className="eyebrow">Kcode native UI</p>
        <h1>Agent operations, memory, context, tools, and self-improvement in one visual surface.</h1>
        <div className="hero-grid">
          <div><b>8</b><span>memory nodes</span></div>
          <div><b>5</b><span>ctx bands</span></div>
          <div><b>5</b><span>tool lanes</span></div>
          <div><b>100%</b><span>local-first scaffold</span></div>
        </div>
      </section>

      <section className="content-grid">
        <MemoryConstellation />

        <section className="glass">
          <div className="section-heading"><div><p className="eyebrow">ctx budget</p><h2>Token pressure map</h2></div><span className="pill">summarize before overflow</span></div>
          <div className="ctx-bars">{ctxBands.map((band) => <div key={band.name} className="ctx-row"><span>{band.name}</span><div><i className={band.tone} style={{ width: `${band.used}%` }}/></div><b>{band.used}%</b></div>)}</div>
        </section>

        <section className="glass">
          <div className="section-heading"><div><p className="eyebrow">tool fabric</p><h2>Execution lanes</h2></div><span className="pill hot">auditable</span></div>
          <div className="tool-list">{toolEvents.map((event) => <article key={event.tool} className={event.status}><b>{event.tool}</b><span>{event.purpose}</span><em>{event.status}{event.ms ? ` · ${event.ms}ms` : ""}</em></article>)}</div>
        </section>

        <section className="glass wide">
          <div className="section-heading"><div><p className="eyebrow">self evolution</p><h2>Improvement loop</h2></div><span className="pill">plan → patch → test → commit → push</span></div>
          <div className="loop">
            {[
              "Observe repo and user intent",
              "Build typed UI and memory model",
              "Visualize ctx, traces, artifacts, risk",
              "Run Vite/TypeScript build gates",
              "Commit and push to icedmoca/kcode",
            ].map((step, i) => <div key={step}><b>{String(i + 1).padStart(2, "0")}</b><span>{step}</span></div>)}
          </div>
        </section>
      </section>
    </main>
  );
}

export default App;
