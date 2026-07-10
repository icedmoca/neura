import { useCallback, useEffect, useMemo, useRef, useState } from "react";

/**
 * Cognition mission control: a read-only window into Neura's semantic memory
 * graph, knowledge sources, predictions, reflections, and sleep pipeline.
 * Data comes from /api/knowledge and /api/knowledge/concept (which the UI
 * server derives from the same files the Rust runtime writes) — every number
 * shown here is inspectable state, not narration.
 */

type EvolutionPoint = {
  at?: string;
  items?: number;
  active_concepts?: number;
  concepts_created?: number;
  concepts_updated?: number;
  concepts_retired?: number;
};

type KnowledgeSource = {
  id: string;
  kind?: string;
  locator?: string;
  items: number;
  concepts: number;
  pending_abstraction: number;
  last_ingest?: string;
  last_report?: Record<string, number> | null;
  history?: EvolutionPoint[];
};

type IntentItem = {
  id: string;
  label: string;
  confidence: number;
  active: boolean;
  updated_at?: string;
};

type GraphNode = {
  id: string;
  label: string;
  tags: string[];
  confidence: number;
  active: boolean;
  degree: number;
  evidence_count: number;
};

type LedgerBlock = {
  index?: number;
  timestamp_ms?: number;
  kind?: string;
  subject?: string;
  summary?: string;
  score?: number | null;
  passed?: boolean | null;
};

type KnowledgeEvent = {
  timestamp?: string;
  event?: string;
  detail?: Record<string, unknown> | null;
};

type KnowledgeState = {
  available: boolean;
  reason?: string;
  graph_file?: string;
  totals?: {
    concepts: number;
    active: number;
    tags: number;
    communities: number;
    edge_kinds: Record<string, number>;
    confidence: { low: number; mid: number; high: number };
  };
  sources?: KnowledgeSource[];
  prediction_stats?: {
    reflections?: number;
    predicted_total?: number;
    confirmed_total?: number;
    precision_ewma?: number;
  } | null;
  last_sleep?: Record<string, unknown> | null;
  consolidations?: { semantic_id: string; concept: string; at: string; sources: string[] }[];
  goals?: IntentItem[];
  decisions?: IntentItem[];
  plans?: IntentItem[];
  nodes?: GraphNode[];
  ledger?: LedgerBlock[];
  reflections?: LedgerBlock[];
  events?: KnowledgeEvent[];
};

type EdgeRef = {
  kind: string;
  target?: string;
  source?: string;
  label: string;
  weight: number;
  confidence: number;
};

type ConceptDetail = {
  available: boolean;
  id: string;
  content: string;
  tags: string[];
  confidence: number;
  strength: number;
  access_count: number;
  active: boolean;
  source?: string | null;
  created_at?: string;
  updated_at?: string;
  communities: string[];
  evidence: { kind?: string; id?: string; note?: string; at?: string }[];
  edges_out: EdgeRef[];
  edges_in: EdgeRef[];
};

type TabId = "overview" | "graph" | "timeline" | "predictions" | "sleep";

const TABS: { id: TabId; label: string }[] = [
  { id: "overview", label: "overview" },
  { id: "graph", label: "graph explorer" },
  { id: "timeline", label: "cognition timeline" },
  { id: "predictions", label: "prediction vs reality" },
  { id: "sleep", label: "sleep cycle" },
];

/** Map raw event / ledger kinds onto cognition-loop stages. */
function stageFor(kind: string): string {
  const k = kind.toLowerCase();
  if (k.includes("turn_brief")) return "reasoning";
  if (k.includes("prediction")) return "prediction";
  if (k.includes("plan")) return "planning";
  if (k.includes("decision")) return "decision";
  if (k.includes("ingest") || k.includes("goal_sync")) return "knowledge";
  if (k.includes("tool_evidence") || k.includes("toolinvocation")) return "execution";
  if (k.includes("verification") || k.includes("validation")) return "verification";
  if (k.includes("reflection")) return "reflection";
  if (k.includes("insight")) return "observation";
  if (k.includes("abstracted") || k.includes("sleep")) return "sleep";
  if (k.includes("selfimprovement") || k.includes("rankedtask") || k.includes("patchgate")) return "improvement";
  if (k.includes("adversarialeval") || k.includes("operationaleval")) return "evaluation";
  return "memory";
}

/** Turn raw event details into a sentence a person can read at a glance. */
function humanizeDetail(title: string, detail: unknown): string {
  if (detail == null) return "";
  if (typeof detail === "string") return detail.slice(0, 160);
  if (typeof detail !== "object") return String(detail);
  const d = detail as Record<string, unknown>;
  const n = (key: string): number => Number(d[key] ?? 0);
  const t = title.toLowerCase();
  if (t.includes("ingest")) {
    const parts: string[] = [];
    if (n("concepts_created")) parts.push(`+${n("concepts_created")} concepts`);
    if (n("concepts_updated")) parts.push(`${n("concepts_updated")} updated`);
    if (n("concepts_retired")) parts.push(`${n("concepts_retired")} retired`);
    if (n("edges_added")) parts.push(`+${n("edges_added")} edges`);
    if (parts.length === 0) parts.push("no changes");
    return `${parts.join(", ")} · ${n("items_changed")} item(s) in ${n("duration_ms")}ms`;
  }
  if (t.includes("reflection")) {
    return `${n("confirmed")}/${n("predicted_concepts")} predicted concepts confirmed (${Math.round(n("precision") * 100)}% precision)`;
  }
  if (t.includes("prediction")) {
    const concepts = Array.isArray(d.predicted_concepts) ? d.predicted_concepts.length : 0;
    const preview = typeof d.query_preview === "string" ? ` for “${d.query_preview.slice(0, 48)}”` : "";
    return `expects ${concepts} concept(s) to be touched${preview}`;
  }
  if (t.includes("plan")) {
    const topic = typeof d.topic === "string" ? `“${d.topic}” — ` : "";
    return `${topic}${n("stages")} stage(s), complexity ${n("complexity").toFixed(1)}, uncertainty ${n("uncertainty").toFixed(2)}`;
  }
  if (t.includes("decision")) {
    return typeof d.decision === "string" ? `“${d.decision}”` : "";
  }
  if (t.includes("turn_brief")) return `architectural context injected (${n("chars")} chars)`;
  if (t.includes("goal_sync")) return `${n("goals")} goal(s), ${n("links")} architectural link(s)`;
  if (t.includes("verification")) return `${d.passed ? "passed" : "failed"} (${n("checks")} checks)`;
  if (t.includes("insights")) return `${n("count")} observation(s) recorded`;
  if (t.includes("tool_evidence")) return `${n("applied")} tool outcome(s) folded into concepts`;
  const s = JSON.stringify(d);
  return s.length > 140 ? `${s.slice(0, 140)}…` : s;
}

function pct(v: number | null | undefined): string {
  return v == null ? "–" : `${Math.round(v * 100)}%`;
}

function shortTime(iso?: string, ms?: number): string {
  const d = iso ? new Date(iso) : ms ? new Date(ms) : null;
  if (!d || Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function Sparkline({ points }: { points: number[] }) {
  if (points.length < 2) return <span className="cog-muted">–</span>;
  const w = 120;
  const h = 24;
  const max = Math.max(...points, 1);
  const min = Math.min(...points);
  const span = Math.max(max - min, 1);
  const step = w / (points.length - 1);
  const path = points
    .map((v, i) => `${i === 0 ? "M" : "L"}${(i * step).toFixed(1)},${(h - ((v - min) / span) * (h - 2) - 1).toFixed(1)}`)
    .join(" ");
  return (
    <svg width={w} height={h} className="cog-spark" aria-hidden>
      <path d={path} fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

function Meter({ value, label }: { value: number; label: string }) {
  return (
    <div className="cog-meter" title={label}>
      <div className="cog-meter__bar">
        <div className="cog-meter__fill" style={{ width: `${Math.round(Math.min(Math.max(value, 0), 1) * 100)}%` }} />
      </div>
      <span className="cog-meter__label">{label}</span>
    </div>
  );
}

/** Radial neighbor map: selected concept at center, typed edges around it. */
function NeighborMap({
  detail,
  onNavigate,
}: {
  detail: ConceptDetail;
  onNavigate: (id: string) => void;
}) {
  const neighbors = useMemo(() => {
    const seen = new Set<string>();
    const merged: { id: string; kind: string; label: string; dir: "out" | "in" }[] = [];
    for (const e of detail.edges_out) {
      const id = e.target ?? "";
      if (id && !seen.has(id)) {
        seen.add(id);
        merged.push({ id, kind: e.kind, label: e.label, dir: "out" });
      }
    }
    for (const e of detail.edges_in) {
      const id = e.source ?? "";
      if (id && !seen.has(id)) {
        seen.add(id);
        merged.push({ id, kind: e.kind, label: e.label, dir: "in" });
      }
    }
    return merged.slice(0, 12);
  }, [detail]);

  if (neighbors.length === 0) return null;
  const size = 460;
  const cx = size / 2;
  const cy = 150;
  const r = 110;

  return (
    <svg viewBox={`0 0 ${size} 300`} className="cog-map" role="img" aria-label="concept neighborhood">
      {neighbors.map((n, i) => {
        const angle = (i / neighbors.length) * Math.PI * 2 - Math.PI / 2;
        const x = cx + Math.cos(angle) * r;
        const y = cy + Math.sin(angle) * (r * 0.72);
        const mx = (cx + x) / 2;
        const my = (cy + y) / 2;
        return (
          <g key={n.id} className="cog-map__node" onClick={() => onNavigate(n.id)}>
            <line x1={cx} y1={cy} x2={x} y2={y} className={`cog-map__edge cog-map__edge--${n.dir}`} />
            <text x={mx} y={my - 3} textAnchor="middle" className="cog-map__kind">
              {n.dir === "in" ? `←${n.kind}` : n.kind}
            </text>
            <circle cx={x} cy={y} r={5} className="cog-map__dot" />
            <text
              x={x}
              y={y + (Math.sin(angle) >= 0 ? 18 : -12)}
              textAnchor="middle"
              className="cog-map__label"
            >
              {n.label.slice(0, 30)}
            </text>
          </g>
        );
      })}
      <circle cx={cx} cy={cy} r={8} className="cog-map__center" />
      <text x={cx} y={cy - 16} textAnchor="middle" className="cog-map__center-label">
        {detail.content.split("\n")[0]?.slice(0, 44)}
      </text>
    </svg>
  );
}

export function CognitionPanel({
  projectPath,
  onClose,
}: {
  projectPath: string | null;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<TabId>("overview");
  const [data, setData] = useState<KnowledgeState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<ConceptDetail | null>(null);
  const [search, setSearch] = useState("");
  const [trail, setTrail] = useState<string[]>([]);
  const timerRef = useRef<number | null>(null);

  const load = useCallback(async () => {
    try {
      const q = projectPath ? `?project=${encodeURIComponent(projectPath)}` : "";
      const res = await fetch(`/api/knowledge${q}`, { cache: "no-store" });
      const json = (await res.json()) as KnowledgeState;
      setData(json);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [projectPath]);

  const loadDetail = useCallback(
    async (id: string) => {
      try {
        const q = new URLSearchParams();
        if (projectPath) q.set("project", projectPath);
        q.set("id", id);
        const res = await fetch(`/api/knowledge/concept?${q.toString()}`, { cache: "no-store" });
        const json = (await res.json()) as ConceptDetail;
        if (json.available) setDetail(json);
      } catch {
        /* keep prior detail */
      }
    },
    [projectPath],
  );

  const navigate = useCallback(
    (id: string) => {
      setSelectedId(id);
      setTrail((prev) => [...prev.filter((t) => t !== id), id].slice(-8));
      void loadDetail(id);
      setTab("graph");
    },
    [loadDetail],
  );

  useEffect(() => {
    void load();
    timerRef.current = window.setInterval(() => void load(), 6000);
    return () => {
      if (timerRef.current !== null) window.clearInterval(timerRef.current);
    };
  }, [load]);

  useEffect(() => {
    // Preselect the highest-degree concept without stealing the active tab.
    if (!selectedId && data?.nodes?.length) {
      const id = data.nodes[0].id;
      setSelectedId(id);
      setTrail([id]);
      void loadDetail(id);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data?.nodes]);

  const filteredNodes = useMemo(() => {
    const nodes = data?.nodes ?? [];
    const needle = search.trim().toLowerCase();
    if (!needle) return nodes;
    return nodes.filter(
      (n) => n.label.toLowerCase().includes(needle) || n.tags.some((t) => t.toLowerCase().includes(needle)),
    );
  }, [data?.nodes, search]);

  const timeline = useMemo(() => {
    const items: { at: number; stage: string; title: string; detail: string; count: number }[] = [];
    for (const e of data?.events ?? []) {
      const at = e.timestamp ? new Date(e.timestamp).getTime() : 0;
      const title = (e.event ?? "").replace("knowledge_", "");
      items.push({
        at,
        stage: stageFor(e.event ?? ""),
        title,
        detail: humanizeDetail(title, e.detail),
        count: 1,
      });
    }
    for (const b of data?.ledger ?? []) {
      items.push({
        at: b.timestamp_ms ?? 0,
        stage: stageFor(b.kind ?? ""),
        title: `${b.kind} #${b.index}`,
        detail: b.summary ?? "",
        count: 1,
      });
    }
    items.sort((a, b) => b.at - a.at);
    // Collapse runs of near-identical entries (maintenance passes repeat the
    // same ingest/eval lines many times) into one row with a ×N badge.
    const collapsed: typeof items = [];
    for (const item of items) {
      const prev = collapsed[collapsed.length - 1];
      const prevTitle = prev?.title.replace(/ #\d+$/, "");
      const itemTitle = item.title.replace(/ #\d+$/, "");
      if (prev && prev.stage === item.stage && prevTitle === itemTitle && prev.detail === item.detail) {
        prev.count += 1;
        continue;
      }
      collapsed.push({ ...item });
    }
    return collapsed.slice(0, 60);
  }, [data?.events, data?.ledger]);

  const goal = data?.goals?.[0];

  return (
    <div className="cog-scrim" onClick={onClose}>
      <div className="cog-panel" onClick={(e) => e.stopPropagation()}>
        <header className="cog-head">
          <div className="cog-head__title">
            <span className="cog-head__brand">COGNITION</span>
            {data?.graph_file && <span className="cog-muted">graph {data.graph_file}</span>}
          </div>
          <nav className="cog-tabs">
            {TABS.map((t) => (
              <button
                key={t.id}
                type="button"
                className={`cog-tab ${tab === t.id ? "cog-tab--on" : ""}`}
                onClick={() => setTab(t.id)}
              >
                {t.label}
              </button>
            ))}
          </nav>
          <button type="button" className="icon-btn" title="Close" onClick={onClose}>✕</button>
        </header>

        {error && <div className="cog-empty">state fetch failed: {error}</div>}
        {data && !data.available && (
          <div className="cog-empty">
            No knowledge graph yet for this project.
            <br />
            <code>neura knowledge ingest {projectPath ?? "."}</code> builds one; sleep keeps it live.
          </div>
        )}

        {data?.available && tab === "overview" && (
          <div className="cog-body">
            <section className="cog-goalstrip">
              <span className="cog-muted">current goal</span>
              {goal ? (
                <>
                  <strong>{goal.label}</strong>
                  <Meter value={goal.confidence} label={`confidence ${pct(goal.confidence)}`} />
                </>
              ) : (
                <strong className="cog-muted">none recorded — goals mirror in from the goal tool</strong>
              )}
            </section>

            <div className="cog-grid">
              <section className="cog-card">
                <h4>knowledge sources</h4>
                {(data.sources ?? []).length === 0 && <div className="cog-muted">none registered</div>}
                {(data.sources ?? []).map((s) => (
                  <div key={s.id} className="cog-source">
                    <div className="cog-source__head">
                      <strong>{s.locator?.split("/").pop() ?? s.id}</strong>
                      <span className="cog-muted">{s.kind}</span>
                    </div>
                    <div className="cog-source__stats">
                      {s.concepts} concepts · {s.items} items · {s.pending_abstraction} awaiting abstraction
                    </div>
                    <Sparkline points={(s.history ?? []).map((p) => p.active_concepts ?? 0)} />
                  </div>
                ))}
                <h4>graph</h4>
                <div className="cog-chips">
                  <span className="cog-chip">{data.totals?.active} active concepts</span>
                  <span className="cog-chip">{data.totals?.communities} communities</span>
                  {Object.entries(data.totals?.edge_kinds ?? {})
                    .sort((a, b) => b[1] - a[1])
                    .slice(0, 6)
                    .map(([k, n]) => (
                      <span key={k} className="cog-chip cog-chip--dim">{k} {n}</span>
                    ))}
                </div>
                <div className="cog-confbar" title="concept confidence distribution">
                  {(["high", "mid", "low"] as const).map((band) => {
                    const c = data.totals?.confidence ?? { low: 0, mid: 0, high: 0 };
                    const total = Math.max(c.low + c.mid + c.high, 1);
                    return (
                      <div
                        key={band}
                        className={`cog-confbar__seg cog-confbar__seg--${band}`}
                        style={{ width: `${(c[band] / total) * 100}%` }}
                        title={`${band} confidence: ${c[band]}`}
                      />
                    );
                  })}
                </div>
              </section>

              <section className="cog-card">
                <h4>engineering intent</h4>
                <div className="cog-intent">
                  <span className="cog-muted">decisions</span>
                  {(data.decisions ?? []).length === 0 && <div className="cog-muted">none recorded</div>}
                  {(data.decisions ?? []).map((d) => (
                    <button key={d.id} type="button" className="cog-link" onClick={() => navigate(d.id)}>
                      [{pct(d.confidence)}] {d.label}
                    </button>
                  ))}
                  <span className="cog-muted">plans</span>
                  {(data.plans ?? []).length === 0 && <div className="cog-muted">none yet</div>}
                  {(data.plans ?? []).map((p) => (
                    <button key={p.id} type="button" className="cog-link" onClick={() => navigate(p.id)}>
                      {p.label}
                    </button>
                  ))}
                </div>
              </section>

              <section className="cog-card">
                <h4>calibration</h4>
                {data.prediction_stats?.reflections ? (
                  <>
                    <Meter
                      value={data.prediction_stats.precision_ewma ?? 0}
                      label={`prediction precision ${pct(data.prediction_stats.precision_ewma)} (EWMA)`}
                    />
                    <div className="cog-muted">
                      {data.prediction_stats.confirmed_total}/{data.prediction_stats.predicted_total} predicted
                      concepts confirmed over {data.prediction_stats.reflections} reflection(s)
                    </div>
                  </>
                ) : (
                  <div className="cog-muted">no reflections yet — predictions score when work lands</div>
                )}
                <h4>last sleep</h4>
                {data.last_sleep ? (
                  <div className="cog-chips">
                    {Object.entries(data.last_sleep)
                      .filter(([k, v]) => typeof v === "number" && v !== 0 && k !== "at")
                      .map(([k, v]) => (
                        <span key={k} className="cog-chip cog-chip--dim">{k.replaceAll("_", " ")} {String(v)}</span>
                      ))}
                  </div>
                ) : (
                  <div className="cog-muted">no sleep cycle recorded</div>
                )}
              </section>
            </div>
          </div>
        )}

        {data?.available && tab === "graph" && (
          <div className="cog-body cog-body--split">
            <aside className="cog-nodes">
              <input
                className="cog-search"
                placeholder="search concepts…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <div className="cog-nodes__list">
                {filteredNodes.map((n) => (
                  <button
                    key={n.id}
                    type="button"
                    className={`cog-node ${selectedId === n.id ? "cog-node--on" : ""} ${n.active ? "" : "cog-node--retired"}`}
                    onClick={() => navigate(n.id)}
                  >
                    <span className="cog-node__degree">{n.degree}</span>
                    <span className="cog-node__label">{n.label}</span>
                  </button>
                ))}
              </div>
            </aside>
            <section className="cog-detail">
              {detail ? (
                <>
                  {trail.length > 1 && (
                    <div className="cog-trail">
                      {trail.map((id) => (
                        <button
                          key={id}
                          type="button"
                          className={`cog-trail__crumb ${id === selectedId ? "cog-trail__crumb--on" : ""}`}
                          onClick={() => navigate(id)}
                          title={id}
                        >
                          {id.slice(-8)}
                        </button>
                      ))}
                    </div>
                  )}
                  <NeighborMap detail={detail} onNavigate={navigate} />
                  <pre className="cog-content">{detail.content}</pre>
                  <div className="cog-chips">
                    <span className="cog-chip">confidence {pct(detail.confidence)}</span>
                    <span className="cog-chip">strength {detail.strength}</span>
                    <span className="cog-chip">{detail.active ? "active" : "retired"}</span>
                    {detail.communities.map((c) => (
                      <span key={c} className="cog-chip cog-chip--dim">community {c}</span>
                    ))}
                    {detail.tags.slice(0, 6).map((t) => (
                      <span key={t} className="cog-chip cog-chip--dim">#{t}</span>
                    ))}
                  </div>
                  <h4>evidence chain ({detail.evidence.length})</h4>
                  <ul className="cog-evidence">
                    {detail.evidence.map((ev, i) => (
                      <li key={i}>
                        <span className="cog-muted">{shortTime(ev.at)} {ev.kind}</span> {ev.note || ev.id}
                      </li>
                    ))}
                  </ul>
                </>
              ) : (
                <div className="cog-muted">select a concept</div>
              )}
            </section>
          </div>
        )}

        {data?.available && tab === "timeline" && (
          <div className="cog-body">
            <div className="cog-timeline">
              {timeline.length === 0 && <div className="cog-muted">no cognition events yet</div>}
              {timeline.map((item, i) => (
                <div key={i} className="cog-tl">
                  <span className="cog-tl__time">{shortTime(undefined, item.at)}</span>
                  <span className={`cog-tl__stage cog-tl__stage--${item.stage}`}>{item.stage}</span>
                  <span className="cog-tl__title">
                    {item.title}
                    {item.count > 1 && <span className="cog-tl__count"> ×{item.count}</span>}
                  </span>
                  <span className="cog-tl__detail cog-muted" title={item.detail}>{item.detail}</span>
                </div>
              ))}
            </div>
          </div>
        )}

        {data?.available && tab === "predictions" && (
          <div className="cog-body">
            {data.prediction_stats?.reflections ? (
              <section className="cog-card">
                <Meter
                  value={data.prediction_stats.precision_ewma ?? 0}
                  label={`rolling precision ${pct(data.prediction_stats.precision_ewma)} · ${data.prediction_stats.confirmed_total}/${data.prediction_stats.predicted_total} concepts confirmed · ${data.prediction_stats.reflections} reflections`}
                />
              </section>
            ) : (
              <div className="cog-empty">
                No prediction-vs-reality data yet. Neura records an architectural expectation with every
                turn brief; when edits land and evidence folds in (sleep / sync), each expectation is scored
                here — confirmed concepts strengthen, misses become evidence.
              </div>
            )}
            {(data.reflections ?? []).map((r) => (
              <section key={r.index} className="cog-card cog-reflection">
                <div className="cog-reflection__head">
                  <span>#{r.index} {shortTime(undefined, r.timestamp_ms)}</span>
                  <span className={r.passed ? "" : "cog-muted"}>{r.passed ? "confirmed" : "missed"}</span>
                </div>
                <div>{r.summary}</div>
                {typeof r.score === "number" && <Meter value={r.score} label={`precision ${pct(r.score)}`} />}
              </section>
            ))}
          </div>
        )}

        {data?.available && tab === "sleep" && (
          <div className="cog-body">
            {data.last_sleep ? (
              <section className="cog-card">
                <h4>last sleep cycle {typeof data.last_sleep.at === "string" ? `· ${shortTime(data.last_sleep.at)}` : ""}</h4>
                <div className="cog-sleep">
                  {[
                    ["linked", "associations grown"],
                    ["weakened", "associations faded"],
                    ["pruned", "associations pruned"],
                    ["communities", "communities detected"],
                    ["consolidated", "concepts consolidated"],
                    ["contradictions_found", "contradictions found"],
                    ["concept_embeddings_refreshed", "concept embeddings refreshed"],
                    ["confidence_decayed", "confidence re-weighted"],
                    ["knowledge_concepts_refreshed", "repository knowledge refreshed"],
                  ].map(([key, label]) => (
                    <div key={key} className="cog-sleep__row">
                      <span className="cog-sleep__count">{String(data.last_sleep?.[key] ?? 0)}</span>
                      <span>{label}</span>
                    </div>
                  ))}
                </div>
              </section>
            ) : (
              <div className="cog-empty">
                No sleep cycle recorded yet — <code>neura memory sleep</code> runs consolidation, community
                detection, contradiction review, and concept-embedding refresh over this graph.
              </div>
            )}
            {(data.consolidations ?? []).length > 0 && (
              <section className="cog-card">
                <h4>recent consolidations</h4>
                {(data.consolidations ?? []).map((c, i) => (
                  <button key={i} type="button" className="cog-link" onClick={() => navigate(c.semantic_id)}>
                    {shortTime(c.at)} · “{c.concept}” from {c.sources.length} episodes
                  </button>
                ))}
              </section>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

export default CognitionPanel;
