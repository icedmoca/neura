import { useMemo, useState } from 'react'
import './index.css'

type Message = {
  role: 'user' | 'assistant'
  text: string
  time: string
}

type MemoryNode = {
  id: string
  label: string
  kind: 'goal' | 'decision' | 'artifact' | 'risk'
}

type MemoryEdge = {
  from: string
  to: string
  label: string
}

type Subchat = {
  id: string
  title: string
  purpose: string
  status: 'active' | 'paused' | 'done'
  tokens: string
  messages: Message[]
  memories: MemoryNode[]
  edges: MemoryEdge[]
}

type Project = {
  id: string
  title: string
  summary: string
  color: string
  updated: string
  subchats: Subchat[]
}

const projects: Project[] = [
  {
    id: 'neura-web',
    title: 'Neura Web Console',
    summary: 'Design, ship, and observe the browser workspace.',
    color: '#7c3aed',
    updated: '2 min ago',
    subchats: [
      {
        id: 'overview',
        title: 'Project overview',
        purpose: 'Keep the project brief, constraints, and next milestones aligned.',
        status: 'active',
        tokens: '14.2k',
        messages: [
          { role: 'user', text: 'Make main chats feel like projects and let each project branch into focused subchats.', time: '03:18' },
          { role: 'assistant', text: 'I will model projects as the durable workspace and subchats as linked workstreams with shared memory.', time: '03:19' },
        ],
        memories: [
          { id: 'goal', label: 'Project = main chat', kind: 'goal' },
          { id: 'subs', label: 'Subchats branch by task', kind: 'decision' },
          { id: 'graph', label: 'Shared graph memory', kind: 'artifact' },
          { id: 'scope', label: 'Avoid context drift', kind: 'risk' },
        ],
        edges: [
          { from: 'goal', to: 'subs', label: 'decomposes into' },
          { from: 'subs', to: 'graph', label: 'writes to' },
          { from: 'graph', to: 'scope', label: 'guards against' },
        ],
      },
      {
        id: 'memory',
        title: 'Graph memory',
        purpose: 'Capture reusable facts, decisions, files, and relationships across subchats.',
        status: 'active',
        tokens: '8.7k',
        messages: [
          { role: 'assistant', text: 'Memory nodes should be visible, filterable, and attached to the active subchat.', time: '03:12' },
          { role: 'user', text: 'I want to see how each subchat contributes to the project memory.', time: '03:15' },
        ],
        memories: [
          { id: 'facts', label: 'Facts', kind: 'artifact' },
          { id: 'decisions', label: 'Decisions', kind: 'decision' },
          { id: 'files', label: 'Files', kind: 'artifact' },
          { id: 'owners', label: 'Owners', kind: 'goal' },
        ],
        edges: [
          { from: 'facts', to: 'decisions', label: 'supports' },
          { from: 'decisions', to: 'files', label: 'changes' },
          { from: 'owners', to: 'decisions', label: 'approved' },
        ],
      },
      {
        id: 'release',
        title: 'Release polish',
        purpose: 'Track final UX refinements, quality gates, and launch notes.',
        status: 'paused',
        tokens: '3.1k',
        messages: [
          { role: 'assistant', text: 'Pending validation: responsive layout, empty states, and keyboard flow.', time: 'Yesterday' },
        ],
        memories: [
          { id: 'qa', label: 'QA checklist', kind: 'goal' },
          { id: 'mobile', label: 'Mobile nav', kind: 'risk' },
          { id: 'copy', label: 'Launch copy', kind: 'artifact' },
        ],
        edges: [
          { from: 'qa', to: 'mobile', label: 'covers' },
          { from: 'qa', to: 'copy', label: 'includes' },
        ],
      },
    ],
  },
  {
    id: 'agent-runtime',
    title: 'Agent Runtime',
    summary: 'Improve autonomous coding, shell safety, and verification loops.',
    color: '#0891b2',
    updated: '1 hr ago',
    subchats: [
      {
        id: 'verification',
        title: 'Verification loops',
        purpose: 'Design measurable checks before claiming work is complete.',
        status: 'done',
        tokens: '21.9k',
        messages: [{ role: 'assistant', text: 'Every implementation should have an explicit validation path.', time: '10:40' }],
        memories: [
          { id: 'tests', label: 'Tests first', kind: 'decision' },
          { id: 'metrics', label: 'Metrics', kind: 'artifact' },
        ],
        edges: [{ from: 'tests', to: 'metrics', label: 'produce' }],
      },
    ],
  },
]

function App() {
  const [selectedProjectId, setSelectedProjectId] = useState(projects[0].id)
  const selectedProject = projects.find((project) => project.id === selectedProjectId) ?? projects[0]
  const [selectedSubchatId, setSelectedSubchatId] = useState(selectedProject.subchats[0].id)

  const activeSubchat = useMemo(() => {
    return selectedProject.subchats.find((subchat) => subchat.id === selectedSubchatId) ?? selectedProject.subchats[0]
  }, [selectedProject, selectedSubchatId])

  const selectProject = (project: Project) => {
    setSelectedProjectId(project.id)
    setSelectedSubchatId(project.subchats[0].id)
  }

  return (
    <main className="app-shell">
      <aside className="project-rail" aria-label="Projects">
        <div className="brand-card">
          <span className="brand-mark">N</span>
          <div>
            <p className="eyebrow">Neura workspace</p>
            <h1>Project chats</h1>
          </div>
        </div>

        <button className="new-project">+ New project</button>

        <div className="project-list">
          {projects.map((project) => (
            <button
              key={project.id}
              className={`project-card ${project.id === selectedProject.id ? 'selected' : ''}`}
              onClick={() => selectProject(project)}
              style={{ '--accent': project.color } as React.CSSProperties}
            >
              <span className="project-dot" />
              <span>
                <strong>{project.title}</strong>
                <small>{project.summary}</small>
              </span>
              <em>{project.updated}</em>
            </button>
          ))}
        </div>
      </aside>

      <section className="workspace">
        <header className="workspace-header">
          <div>
            <p className="eyebrow">Main chat as project</p>
            <h2>{selectedProject.title}</h2>
            <p>{selectedProject.summary}</p>
          </div>
          <div className="header-actions">
            <button>Share</button>
            <button className="primary">+ Subchat</button>
          </div>
        </header>

        <div className="workspace-grid">
          <nav className="subchat-panel" aria-label="Subchats">
            <div className="panel-title">
              <span>Subchats</span>
              <strong>{selectedProject.subchats.length}</strong>
            </div>
            {selectedProject.subchats.map((subchat, index) => (
              <button
                key={subchat.id}
                className={`subchat-card ${subchat.id === activeSubchat.id ? 'selected' : ''}`}
                onClick={() => setSelectedSubchatId(subchat.id)}
              >
                <span className="branch-index">{index + 1}</span>
                <span className="subchat-copy">
                  <strong>{subchat.title}</strong>
                  <small>{subchat.purpose}</small>
                </span>
                <span className={`status ${subchat.status}`}>{subchat.status}</span>
              </button>
            ))}
          </nav>

          <section className="chat-stage">
            <div className="chat-titlebar">
              <div>
                <p className="eyebrow">Focused branch</p>
                <h3>{activeSubchat.title}</h3>
                <span>{activeSubchat.purpose}</span>
              </div>
              <div className="token-pill">{activeSubchat.tokens} context</div>
            </div>

            <div className="message-stack">
              {activeSubchat.messages.map((message, index) => (
                <article key={`${message.time}-${index}`} className={`message ${message.role}`}>
                  <div className="avatar">{message.role === 'assistant' ? 'N' : 'U'}</div>
                  <div>
                    <div className="message-meta">
                      <strong>{message.role === 'assistant' ? 'Neura' : 'You'}</strong>
                      <time>{message.time}</time>
                    </div>
                    <p>{message.text}</p>
                  </div>
                </article>
              ))}
            </div>

            <form className="composer">
              <input placeholder={`Ask inside “${activeSubchat.title}”...`} aria-label="Message subchat" />
              <button type="button">Attach memory</button>
              <button type="submit" className="primary">Send</button>
            </form>
          </section>

          <aside className="memory-panel" aria-label="Graph memory">
            <div className="panel-title">
              <span>Graphed memory</span>
              <strong>{activeSubchat.memories.length} nodes</strong>
            </div>

            <div className="memory-canvas">
              {activeSubchat.memories.map((node, index) => (
                <div key={node.id} className={`memory-node ${node.kind}`} style={{ '--i': index } as React.CSSProperties}>
                  {node.label}
                </div>
              ))}
            </div>

            <div className="edge-list">
              {activeSubchat.edges.map((edge) => (
                <div key={`${edge.from}-${edge.to}`} className="edge-row">
                  <span>{edge.from}</span>
                  <small>{edge.label}</small>
                  <span>{edge.to}</span>
                </div>
              ))}
            </div>
          </aside>
        </div>
      </section>
    </main>
  )
}

export default App
