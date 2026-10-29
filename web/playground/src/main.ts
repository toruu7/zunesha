// Zunesha playground entry point.
//
// Read-only: the page consumes the compact `/api/playground` projection and
// the `/api/playground/ws` stream. It never posts actions, never receives logs
// or diffs, and never renders source code.

import './style.css';
import { AgentTracker, type PlaygroundAgent } from './agents';
import { Renderer } from './renderer';

interface PlaygroundSnapshot {
  project: { name: string; root: string };
  agents: PlaygroundAgent[];
  running: number;
}

const tracker = new AgentTracker();
const canvas = document.getElementById('office') as HTMLCanvasElement;
const renderer = new Renderer(canvas, tracker);

const statusEl = document.getElementById('status')!;
const projectEl = document.getElementById('project-name')!;
const projectRootEl = document.getElementById('project-root')!;
const countEl = document.getElementById('agent-count')!;
const workingEl = document.getElementById('working-count')!;
const waitingEl = document.getElementById('waiting-count')!;
const sleepingEl = document.getElementById('sleeping-count')!;
const emptyEl = document.getElementById('empty')!;

let socketOpen = false;

function setStatus(text: string, connected: boolean): void {
  statusEl.textContent = text;
  statusEl.className = connected ? 'status connected' : 'status disconnected';
}

function updateSummary(): void {
  const agents = tracker.getAll();
  const working = agents.filter((agent) => agent.visualState === 'working').length;
  const waiting = agents.filter((agent) => agent.visualState === 'waiting').length;
  const sleeping = agents.filter((agent) => agent.visualState === 'sleeping').length;
  countEl.textContent = String(agents.length);
  workingEl.textContent = String(working);
  waitingEl.textContent = String(waiting);
  sleepingEl.textContent = String(sleeping);
  emptyEl.hidden = agents.length > 0;
}

function applySnapshot(snapshot: PlaygroundSnapshot): void {
  tracker.setSnapshot(snapshot.agents);
  projectEl.textContent = snapshot.project.name;
  projectRootEl.textContent = snapshot.project.root;
  projectRootEl.title = snapshot.project.root;
  renderer.setProject(snapshot.project.name);
  updateSummary();
}

async function loadInitial(): Promise<void> {
  try {
    const response = await fetch('/api/playground');
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    applySnapshot(await response.json());
    if (!socketOpen) setStatus('snapshot loaded', true);
  } catch (error) {
    setStatus('daemon offline — retrying', false);
    console.warn('[playground] initial snapshot failed:', error);
  }
}

function connect(): void {
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const socket = new WebSocket(`${protocol}//${location.host}/api/playground/ws`);

  socket.addEventListener('open', () => {
    socketOpen = true;
    setStatus('live', true);
  });

  socket.addEventListener('message', (event) => {
    let message: { type?: string; project?: PlaygroundSnapshot['project']; agents?: PlaygroundAgent[]; agent?: PlaygroundAgent };
    try {
      message = JSON.parse(event.data);
    } catch {
      return;
    }
    if (message.type === 'snapshot' && message.project && message.agents) {
      applySnapshot({ project: message.project, agents: message.agents, running: 0 });
    } else if (message.type === 'agent_updated' && message.agent) {
      tracker.update(message.agent);
      updateSummary();
    }
  });

  socket.addEventListener('close', () => {
    socketOpen = false;
    setStatus('reconnecting…', false);
    window.setTimeout(connect, 2000);
  });

  socket.addEventListener('error', () => socket.close());
}

renderer.start();
void loadInitial();
connect();
