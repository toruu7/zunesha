// Zunesha office — an interactive control room for the local daemon.
//
// The canvas is the office view; the rail and inspector are ordinary DOM so
// task selection, plan/run/approve/reject/recover, live logs, diffs and final
// Markdown output all work without leaving the page. The page talks to the
// same REST + `/ws` API as the dashboard.

import './style.css';
import { AgentTracker, type PlaygroundAgent } from './agents';
import { Renderer } from './renderer';
import { soundManager } from './audio';
import {
  api,
  eventSocket,
  type ApiEvent,
  type CrewResponse,
  type DependencyEdge,
  type LogEvent,
  type ModelsResponse,
  type ProjectInfo,
  type Skill,
  type StatusCount,
  type Task,
  type TaskStatus,
  type TasksResponse,
  type VersionResponse,
} from './api';
import {
  addAttachmentFiles,
  formatBytes,
  toPayload,
  type AttachmentDraft,
} from './attachments';

const STATUS_LABEL: Record<TaskStatus, string> = {
  BACKLOG: 'backlog',
  TODO: 'ready',
  IN_PROGRESS: 'working',
  EVALUATING: 'validating',
  REVIEW: 'review',
  RESOLVING_CONFLICTS: 'resolving',
  DONE: 'done',
  FAILED_NEEDS_HUMAN: 'needs human',
};

const STATUS_VISUAL: Record<TaskStatus, string> = {
  BACKLOG: 'sleeping',
  TODO: 'idle',
  IN_PROGRESS: 'working',
  EVALUATING: 'working',
  REVIEW: 'waiting',
  RESOLVING_CONFLICTS: 'working',
  DONE: 'done',
  FAILED_NEEDS_HUMAN: 'error',
};

const ACTIVE_STATUSES: TaskStatus[] = ['IN_PROGRESS', 'EVALUATING', 'RESOLVING_CONFLICTS', 'REVIEW'];

const TAB_LABEL = {
  output: 'Output',
  logs: 'Logs',
  diff: 'Diff',
  task: 'Task',
} as const;

type Tab = keyof typeof TAB_LABEL;

interface State {
  project: ProjectInfo | null;
  tasks: Task[];
  dependencies: DependencyEdge[];
  counts: StatusCount[];
  selectedId: string | null;
  filter: string;
  tab: Tab;
  liveLogs: Map<string, Map<number, LogEvent>>;
  connection: 'connecting' | 'live' | 'offline';
  busy: string | null;
  error: string | null;
  crewConfig: CrewResponse | null;
  availableModels: ModelsResponse | null;
  skillsList: Skill[] | null;
  planAttachments: AttachmentDraft[];
  planAttachmentErrors: string[];
  rejectAttachments: AttachmentDraft[];
  rejectAttachmentErrors: string[];
}

const state: State = {
  project: null,
  tasks: [],
  dependencies: [],
  counts: [],
  selectedId: null,
  filter: '',
  tab: 'output',
  liveLogs: new Map(),
  connection: 'connecting',
  busy: null,
  error: null,
  crewConfig: null,
  availableModels: null,
  skillsList: null,
  planAttachments: [],
  planAttachmentErrors: [],
  rejectAttachments: [],
  rejectAttachmentErrors: [],
};

const tracker = new AgentTracker();
const canvas = document.getElementById('office') as HTMLCanvasElement;
const renderer = new Renderer(canvas, tracker);

const projectNameEl = document.getElementById('project-name') as HTMLElement;
const projectRootEl = document.getElementById('project-root') as HTMLElement;
const countsEl = document.getElementById('counts') as HTMLElement;
const connectionEl = document.getElementById('connection') as HTMLElement;
const zouVersionEl = document.getElementById('zou-version') as HTMLElement | null;
const updateBtn = document.getElementById('update-btn') as HTMLButtonElement | null;
const docsLink = document.getElementById('docs-link') as HTMLAnchorElement | null;
const githubLink = document.getElementById('github-link') as HTMLAnchorElement | null;
const officeStatusEl = document.getElementById('office-status') as HTMLElement;
const emptyEl = document.getElementById('empty') as HTMLElement;
const liveTitleEl = document.getElementById('live-title') as HTMLElement;
const liveMetaEl = document.getElementById('live-meta') as HTMLElement;
const liveLogsEl = document.getElementById('live-logs') as HTMLElement;
const taskListEl = document.getElementById('task-list') as HTMLElement;
const taskSummaryEl = document.getElementById('task-summary') as HTMLElement;
const searchEl = document.getElementById('search') as HTMLInputElement;
const promptEl = document.getElementById('prompt') as HTMLTextAreaElement;
const planBtn = document.getElementById('plan-btn') as HTMLButtonElement;
const runAllBtn = document.getElementById('run-all-btn') as HTMLButtonElement;
const inspectorEl = document.getElementById('inspector') as HTMLElement;
const errorEl = document.getElementById('error') as HTMLElement;
const errorTextEl = document.getElementById('error-text') as HTMLElement;
const toastEl = document.getElementById('toast') as HTMLElement;
const modalEl = document.getElementById('modal') as HTMLElement;
const modalTitleEl = document.getElementById('modal-title') as HTMLElement;
const rejectFeedbackEl = document.getElementById('reject-feedback') as HTMLTextAreaElement;
const modalCancelEl = document.getElementById('modal-cancel') as HTMLButtonElement;
const modalSubmitEl = document.getElementById('modal-submit') as HTMLButtonElement;
const attachBtn = document.getElementById('attach-btn') as HTMLButtonElement;
const attachInput = document.getElementById('attach-input') as HTMLInputElement;
const attachHint = document.getElementById('attach-hint') as HTMLElement;
const attachList = document.getElementById('attach-list') as HTMLElement;
const attachErrors = document.getElementById('attach-errors') as HTMLElement;
const rejectAttachBtn = document.getElementById('reject-attach-btn') as HTMLButtonElement;
const rejectAttachInput = document.getElementById('reject-attach-input') as HTMLInputElement;
const rejectAttachHint = document.getElementById('reject-attach-hint') as HTMLElement;
const rejectAttachList = document.getElementById('reject-attach-list') as HTMLElement;
const rejectAttachErrors = document.getElementById('reject-attach-errors') as HTMLElement;

let socket: WebSocket | null = null;
let reconnectTimer: number | null = null;
let rejectTaskId: string | null = null;
let toastTimer: number | null = null;
let renderQueued = false;

// ── helpers ────────────────────────────────────────────────────────────────

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (char) => {
    switch (char) {
      case '&': return '&amp;';
      case '<': return '&lt;';
      case '>': return '&gt;';
      case '"': return '&quot;';
      default: return '&#39;';
    }
  });
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

function statusLabel(status: TaskStatus): string {
  return STATUS_LABEL[status] ?? status.toLowerCase();
}

function statusClass(status: TaskStatus): string {
  return `status-${status.toLowerCase().replace(/_/g, '-')}`;
}

function crewEmoji(member?: string): string {
  if (!member) return '';
  const lower = member.toLowerCase();
  if (lower.includes('luffy')) return '👒';
  if (lower.includes('zoro')) return '⚔️';
  if (lower.includes('nami')) return '🗺️';
  if (lower.includes('usopp')) return '🎯';
  if (lower.includes('sanji')) return '🍳';
  return '🏴‍☠️';
}

function selectedTask(): Task | undefined {
  return state.tasks.find((task) => task.id === state.selectedId);
}

function visibleTasks(): Task[] {
  const query = state.filter.trim().toLowerCase();
  if (!query) return state.tasks;
  return state.tasks.filter((task) =>
    `${task.id} ${task.title} ${task.description} ${task.status}`.toLowerCase().includes(query),
  );
}

function lineCount(logs: string): number {
  if (!logs) return 0;
  const lines = logs.split('\n');
  return lines[lines.length - 1] === '' ? lines.length - 1 : lines.length;
}

function mergedLogs(task: Task): string[] {
  const persisted = task.worker_logs ? task.worker_logs.split('\n') : [];
  if (persisted.length > 0 && persisted[persisted.length - 1] === '') persisted.pop();
  const live = state.liveLogs.get(task.id);
  if (live) {
    for (const [seq, event] of live) {
      if (seq < persisted.length) persisted[seq] = event.line;
      else persisted.push(event.line);
    }
  }
  return persisted;
}

interface CrewProfile {
  name: string;
  role: string;
  specialty: string;
  emoji: string;
  bounty: string;
  quote: string;
  description: string;
}

const CREW_PROFILES: Record<string, CrewProfile> = {
  luffy: {
    name: 'Monkey D. Luffy',
    role: 'Captain & Strategic Lead',
    specialty: 'High-level orchestration, feature architecture, bold initiatives',
    emoji: '👒',
    bounty: '3,000,000,000 ฿',
    quote: "I'm gonna be King of the Pirates! Let's build something epic!",
    description: 'Leads the Straw Hat Pirates. Breaks down complex epics and guides overarching architectural decisions with unshakable optimism.',
  },
  zoro: {
    name: 'Roronoa Zoro',
    role: 'Combatant & Core Logic',
    specialty: 'Deep backend refactoring, data structures, performance slicing',
    emoji: '⚔️',
    bounty: '1,111,000,000 ฿',
    quote: 'Nothing happened. The bugs have been cut down.',
    description: 'Master of Three-Sword Style. Slices through stubborn bugs, concurrency deadlocks, and messy legacy code with zero wasted motion.',
  },
  nami: {
    name: 'Nami',
    role: 'Navigator & Chief Planner',
    specialty: 'DAG task routing, dependency mapping, scope estimation',
    emoji: '🗺️',
    bounty: '366,000,000 ฿',
    quote: 'The weather in this codebase looks stormy, but I have mapped our route!',
    description: 'Expert navigator and cartographer. Dissects feature requests into safe, acyclic task graphs and directs the crew to port.',
  },
  usopp: {
    name: 'Usopp',
    role: 'Sniper & Test Engineer',
    specialty: 'Precision bugfixes, edge case testing, tool crafting',
    emoji: '🎯',
    bounty: '500,000,000 ฿',
    quote: 'I have 8,000 unit tests behind me! Sniped that regression from 2 miles away.',
    description: 'Sharp-eyed sniper and inventor. Crafts tests, catches regression edge cases, and delivers surgical fixes with pinpoint precision.',
  },
  sanji: {
    name: 'Sanji',
    role: 'Cook & UI/UX Specialist',
    specialty: 'Frontend polish, responsive styling, documentation & delightful UX',
    emoji: '🍳',
    bounty: '1,032,000,000 ฿',
    quote: 'Only the finest, well-crafted interfaces and documentation for our users!',
    description: 'Black Leg master chef. Refuses to let sloppy UI or stale documentation leave the kitchen, serving only five-star presentation.',
  },
  chopper: {
    name: 'Tony Tony Chopper',
    role: 'Doctor & Crash Diagnostics',
    specialty: 'Error healing, panic diagnosis, bug triage & test repairs',
    emoji: '🩺',
    bounty: '1,000 ฿',
    quote: 'Calling me a genius doctor won’t make me happy, you bastard! 💕 I will heal this crash!',
    description: 'Doctor and master of Rumble Balls. Diagnoses stack traces, recovers crashed states, and heals panics with surgical accuracy.',
  },
  robin: {
    name: 'Nico Robin',
    role: 'Archaeologist & Architecture Historian',
    specialty: 'Codebase context synthesis, ADRs, documentation & git archaeology',
    emoji: '📜',
    bounty: '930,000,000 ฿',
    quote: 'History holds the secrets of why this was built. I shall uncover the truth.',
    description: 'Archaeologist of Ohara. Reads ancient commits, deciphers complex systems architecture, and writes pristine living documentation.',
  },
  franky: {
    name: 'Franky (Cutty Flam)',
    role: 'Shipwright & Dev Tools Engineer',
    specialty: 'Infrastructure, build pipelines, Docker containers, toolchain & CI/CD',
    emoji: '⚙️',
    bounty: '394,000,000 ฿',
    quote: 'SUUUPER! This build pipeline is fueled by pure cola and unstoppable iron!',
    description: 'Cyborg shipwright of the Thousand Sunny. Forges blazing fast build workflows, hardens containers, and optimizes project infrastructure.',
  },
  brook: {
    name: 'Brook (Soul King)',
    role: 'Musician & Performance Virtuoso',
    specialty: 'Latency profiling, memory benchmarks, allocations & telemetry cadence',
    emoji: '🎻',
    bounty: '383,000,000 ฿',
    quote: 'Yo-ho-ho-ho! This hot-path gives me chills down to the bone — even though I have no skin!',
    description: 'Soul King virtuoso. Listens to execution harmonics, shaves milliseconds off latency, and tunes memory allocation cadence to perfection.',
  },
  jinbe: {
    name: 'Jinbe (Knight of the Sea)',
    role: 'Helmsman & Release Navigator',
    specialty: 'Git rebasing, merge currents, conflict resolution & stable release steering',
    emoji: '🌊',
    bounty: '1,100,000,000 ฿',
    quote: 'Steer through the roughest merge waves with unwavering resolve. I hold the helm.',
    description: 'First Son of the Sea. Navigates treacherous git merge conflicts, steers turbulent rebases, and docks flawless stable releases.',
  },
};

const CREW_MEMBERS = [
  { id: 'luffy', name: 'Luffy', role: 'Captain', title: 'Strategic Lead', emoji: '👒' },
  { id: 'zoro', name: 'Zoro', role: 'Combatant', title: 'Core Logic', emoji: '⚔️' },
  { id: 'nami', name: 'Nami', role: 'Navigator', title: 'Planner', emoji: '🗺️' },
  { id: 'usopp', name: 'Usopp', role: 'Sniper', title: 'QA Scout', emoji: '🎯' },
  { id: 'sanji', name: 'Sanji', role: 'Cook', title: 'UI & Polish', emoji: '🍳' },
  { id: 'chopper', name: 'Chopper', role: 'Doctor', title: 'Crash Triage', emoji: '🩺' },
  { id: 'robin', name: 'Robin', role: 'Archaeologist', title: 'Architecture Docs', emoji: '📜' },
  { id: 'franky', name: 'Franky', role: 'Shipwright', title: 'Dev Tooling', emoji: '⚙️' },
  { id: 'brook', name: 'Brook', role: 'Musician', title: 'Performance', emoji: '🎻' },
  { id: 'jinbe', name: 'Jinbe', role: 'Helmsman', title: 'Rebase & Release', emoji: '🌊' },
] as const;

type CrewId = (typeof CREW_MEMBERS)[number]['id'];

function guessCrewMember(task: Task): CrewId {
  if (task.crew_member) {
    const lower = task.crew_member.toLowerCase();
    for (const m of CREW_MEMBERS) {
      if (lower.includes(m.id)) return m.id;
    }
  }
  const text = `${task.title} ${task.description}`.toLowerCase();
  if (text.includes('rebase') || text.includes('merge') || text.includes('conflict') || text.includes('release')) {
    return 'jinbe';
  }
  if (text.includes('docker') || text.includes('ci') || text.includes('build') || text.includes('tooling') || text.includes('infra')) {
    return 'franky';
  }
  if (text.includes('perf') || text.includes('latency') || text.includes('benchmark') || text.includes('throughput') || text.includes('profile')) {
    return 'brook';
  }
  if (text.includes('doc') || text.includes('readme') || text.includes('history') || text.includes('archaeolog') || text.includes('adr')) {
    return 'robin';
  }
  if (text.includes('panic') || text.includes('crash') || text.includes('heal') || text.includes('diagnos') || text.includes('triage')) {
    return 'chopper';
  }
  if (text.includes('test') || text.includes('qa') || text.includes('edge') || text.includes('verify')) {
    return 'usopp';
  }
  if (text.includes('refactor') || text.includes('debt') || text.includes('clean') || text.includes('clippy')) {
    return 'zoro';
  }
  if (text.includes('ui') || text.includes('ux') || text.includes('style') || text.includes('css') || text.includes('component')) {
    return 'sanji';
  }
  if (text.includes('plan') || text.includes('dag') || text.includes('route') || text.includes('navigate')) {
    return 'nami';
  }
  return 'luffy';
}

const DEFAULT_CREW: PlaygroundAgent[] = CREW_MEMBERS.map((m) => ({
  id: m.id,
  name: m.name,
  title: `${m.role} · On Standby`,
  status: 'TODO',
  state: 'idle',
  crew_member: m.id,
  updated_at: '',
  retry_count: 0,
}));

function taskToAgent(task: Task): PlaygroundAgent {
  return {
    id: task.id,
    name: task.title,
    title: task.title,
    status: task.status,
    state: STATUS_VISUAL[task.status] ?? 'idle',
    crew_member: task.crew_member,
    updated_at: task.updated_at,
    retry_count: task.retry_count,
  };
}

function pickDefaultTask(tasks: Task[]): string | null {
  const preferred: TaskStatus[] = [
    'REVIEW',
    'FAILED_NEEDS_HUMAN',
    'IN_PROGRESS',
    'EVALUATING',
    'RESOLVING_CONFLICTS',
    'TODO',
    'BACKLOG',
    'DONE',
  ];
  for (const status of preferred) {
    const task = tasks.find((candidate) => candidate.status === status);
    if (task) return task.id;
  }
  return tasks[0]?.id ?? null;
}

function formatDate(value: string): string {
  if (!value) return '—';
  const parsed = Date.parse(value.includes('T') ? value : `${value.replace(' ', 'T')}Z`);
  return Number.isFinite(parsed) ? new Date(parsed).toLocaleString() : value;
}

// ── Markdown (small, dependency-free, HTML-escaped) ────────────────────────

function inlineMarkdown(value: string): string {
  const codeSpans: string[] = [];
  let text = escapeHtml(value);
  text = text.replace(/`([^`]+)`/g, (_match, code: string) => {
    codeSpans.push(`<code>${code}</code>`);
    return `\u0000${codeSpans.length - 1}\u0000`;
  });
  text = text.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  text = text.replace(/(^|[^*])\*([^*]+)\*/g, '$1<em>$2</em>');
  text = text.replace(
    /\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g,
    '<a href="$2" target="_blank" rel="noreferrer">$1</a>',
  );
  return text.replace(/\u0000(\d+)\u0000/g, (_match, index: string) => codeSpans[Number(index)] ?? '');
}

function renderMarkdown(markdown: string): string {
  if (!markdown.trim()) {
    return '<p class="muted">No final output yet. The agent\'s Markdown answer appears here when it finishes.</p>';
  }

  const html: string[] = [];
  let paragraph: string[] = [];
  let list: { type: 'ul' | 'ol'; items: string[] } | null = null;
  let code: { language: string; lines: string[] } | null = null;

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    html.push(`<p>${paragraph.map(inlineMarkdown).join('<br>')}</p>`);
    paragraph = [];
  };
  const flushList = () => {
    if (!list) return;
    html.push(`<${list.type}>${list.items.map((item) => `<li>${inlineMarkdown(item)}</li>`).join('')}</${list.type}>`);
    list = null;
  };
  const flushCode = () => {
    if (!code) return;
    const language = code.language.replace(/[^a-zA-Z0-9_-]/g, '');
    html.push(
      `<pre class="code-block"><code${language ? ` class="language-${language}"` : ''}>${escapeHtml(code.lines.join('\n'))}</code></pre>`,
    );
    code = null;
  };

  for (const line of markdown.replace(/\r\n/g, '\n').split('\n')) {
    if (code) {
      if (/^```/.test(line.trim())) flushCode();
      else code.lines.push(line);
      continue;
    }
    if (/^```/.test(line.trim())) {
      flushParagraph();
      flushList();
      code = { language: line.trim().slice(3).trim(), lines: [] };
      continue;
    }
    if (!line.trim()) {
      flushParagraph();
      flushList();
      continue;
    }

    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      flushParagraph();
      flushList();
      const level = heading[1].length;
      html.push(`<h${level}>${inlineMarkdown(heading[2])}</h${level}>`);
      continue;
    }

    if (/^(-{3,}|\*{3,})$/.test(line.trim())) {
      flushParagraph();
      flushList();
      html.push('<hr>');
      continue;
    }

    const quote = line.match(/^>\s?(.*)$/);
    if (quote) {
      flushParagraph();
      flushList();
      html.push(`<blockquote>${inlineMarkdown(quote[1])}</blockquote>`);
      continue;
    }

    const unordered = line.match(/^[-*]\s+(.*)$/);
    if (unordered) {
      flushParagraph();
      if (list?.type !== 'ul') {
        flushList();
        list = { type: 'ul', items: [] };
      }
      list.items.push(unordered[1]);
      continue;
    }

    const ordered = line.match(/^\d+\.\s+(.*)$/);
    if (ordered) {
      flushParagraph();
      if (list?.type !== 'ol') {
        flushList();
        list = { type: 'ol', items: [] };
      }
      list.items.push(ordered[1]);
      continue;
    }

    flushList();
    paragraph.push(line);
  }

  flushParagraph();
  flushList();
  flushCode();
  return html.join('\n');
}

function renderDiff(diff: string): string {
  if (!diff.trim()) {
    return '<p class="muted">No diff captured yet. A clean review shows the unified diff here.</p>';
  }
  const lines = diff.split('\n').map((line) => {
    let kind = '';
    if (line.startsWith('+++') || line.startsWith('---')) kind = 'meta';
    else if (line.startsWith('@@')) kind = 'hunk';
    else if (line.startsWith('+')) kind = 'add';
    else if (line.startsWith('-')) kind = 'del';
    return `<span class="diff-line ${kind}">${escapeHtml(line) || ' '}</span>`;
  });
  return `<pre class="diff">${lines.join('')}</pre>`;
}

// ── rendering ──────────────────────────────────────────────────────────────

function renderHeader(): void {
  projectNameEl.textContent = state.project?.name ?? 'connecting…';
  projectRootEl.textContent = state.project?.root ?? '';
  projectRootEl.title = state.project?.root ?? '';

  const counts = new Map<TaskStatus, number>();
  for (const task of state.tasks) counts.set(task.status, (counts.get(task.status) ?? 0) + 1);
  const working = (counts.get('IN_PROGRESS') ?? 0) + (counts.get('EVALUATING') ?? 0) + (counts.get('RESOLVING_CONFLICTS') ?? 0);
  const review = counts.get('REVIEW') ?? 0;
  const done = counts.get('DONE') ?? 0;
  const failed = counts.get('FAILED_NEEDS_HUMAN') ?? 0;

  countsEl.innerHTML = [
    `<span class="count"><b>${state.tasks.length}</b> tasks</span>`,
    `<span class="count working"><b>${working}</b> working</span>`,
    `<span class="count waiting"><b>${review}</b> review</span>`,
    `<span class="count done"><b>${done}</b> done</span>`,
    failed ? `<span class="count error"><b>${failed}</b> needs human</span>` : '',
  ].join('');

  connectionEl.textContent = state.connection === 'live' ? 'live' : state.connection === 'offline' ? 'offline' : 'connecting…';
  connectionEl.className = `connection ${state.connection}`;
}

function renderRail(): void {
  const tasks = visibleTasks();
  taskSummaryEl.textContent = state.filter.trim() ? `${tasks.length} of ${state.tasks.length}` : `${state.tasks.length} tasks`;

  if (tasks.length === 0) {
    taskListEl.innerHTML = `<div class="rail-empty">${
      state.tasks.length === 0 ? 'No tasks yet. Plan a feature to populate the office.' : 'No tasks match this filter.'
    }</div>`;
  } else {
    taskListEl.innerHTML = tasks
      .map((task) => {
        const selected = task.id === state.selectedId ? ' selected' : '';
        const visual = STATUS_VISUAL[task.status] ?? 'idle';
        const crewBadge = task.crew_member ? `<span class="task-crew">${crewEmoji(task.crew_member)} ${escapeHtml(task.crew_member)}</span>` : '';
        return `<button class="task-item${selected}" data-task-id="${escapeHtml(task.id)}" type="button">
          <span class="task-dot ${visual}" aria-hidden="true"></span>
          <span class="task-copy">
            <span class="task-title">${escapeHtml(task.title)}</span>
            <span class="task-meta">${crewBadge}<span class="task-state ${statusClass(task.status)}">${statusLabel(task.status)}</span><span class="task-id">${escapeHtml(task.id.slice(-7))}</span></span>
          </span>
        </button>`;
      })
      .join('');
  }

  const ready = state.tasks.filter((task) => task.status === 'TODO').length;
  runAllBtn.disabled = state.busy !== null || ready === 0;
  runAllBtn.textContent = ready > 0 ? `Run ready (${ready})` : 'Run ready';
  planBtn.disabled = state.busy !== null;
  planBtn.textContent = state.busy === 'planning' ? 'Planning…' : 'Plan';
}

function renderAttachmentSet(
  listEl: HTMLElement,
  hintEl: HTMLElement,
  errorsEl: HTMLElement,
  attachments: AttachmentDraft[],
  errors: string[],
  kind: 'plan' | 'reject',
): void {
  hintEl.textContent = attachments.length
    ? `images or files · ${attachments.length}/10`
    : 'images or files · max 10';
  listEl.innerHTML = attachments
    .map((attachment, index) => {
      const preview = attachment.preview
        ? `<img src="${escapeHtml(attachment.preview)}" alt="" />`
        : '<span class="attach-file">FILE</span>';
      return `<li class="attach-chip">
        ${preview}
        <span class="attach-copy">
          <span class="attach-name" title="${escapeHtml(attachment.name)}">${escapeHtml(attachment.name)}</span>
          <span class="attach-size">${formatBytes(attachment.size)}</span>
        </span>
        <button type="button" class="attach-remove" data-remove-attachment="${kind}" data-index="${index}" aria-label="Remove ${escapeHtml(attachment.name)}">✕</button>
      </li>`;
    })
    .join('');
  errorsEl.innerHTML = errors.map((error) => `<li>${escapeHtml(error)}</li>`).join('');
}

function renderAttachments(): void {
  renderAttachmentSet(
    attachList,
    attachHint,
    attachErrors,
    state.planAttachments,
    state.planAttachmentErrors,
    'plan',
  );
  renderAttachmentSet(
    rejectAttachList,
    rejectAttachHint,
    rejectAttachErrors,
    state.rejectAttachments,
    state.rejectAttachmentErrors,
    'reject',
  );
  attachBtn.disabled = state.busy !== null || state.planAttachments.length >= 10;
  rejectAttachBtn.disabled = state.busy !== null || state.rejectAttachments.length >= 10;
}

const attachmentQueues: Record<'plan' | 'reject', Promise<void>> = {
  plan: Promise.resolve(),
  reject: Promise.resolve(),
};

async function addAttachments(kind: 'plan' | 'reject', files: Iterable<File>): Promise<void> {
  // Serialize reads so two quick drops do not both start from the same list.
  const run = attachmentQueues[kind].then(async () => {
    const existing = kind === 'plan' ? state.planAttachments : state.rejectAttachments;
    const result = await addAttachmentFiles(existing, files);
    if (kind === 'plan') {
      state.planAttachments = result.attachments;
      state.planAttachmentErrors = result.errors;
    } else {
      state.rejectAttachments = result.attachments;
      state.rejectAttachmentErrors = result.errors;
    }
    renderAttachments();
  });
  attachmentQueues[kind] = run.catch(() => undefined);
  await run;
}

function clearAttachments(kind: 'plan' | 'reject'): void {
  if (kind === 'plan') {
    state.planAttachments = [];
    state.planAttachmentErrors = [];
  } else {
    state.rejectAttachments = [];
    state.rejectAttachmentErrors = [];
  }
  renderAttachments();
}

function actionButtons(task: Task): string {
  const busy = state.busy !== null;
  const canRun = task.status === 'TODO';
  const canApprove = task.status === 'REVIEW';
  const canReject = task.status === 'REVIEW' || task.status === 'FAILED_NEEDS_HUMAN';
  const canCancel = task.status === 'IN_PROGRESS' || task.status === 'EVALUATING' || task.status === 'RESOLVING_CONFLICTS';
  const canRecover = canCancel;
  const canRetry = task.status === 'FAILED_NEEDS_HUMAN' || task.status === 'REVIEW' || task.status === 'DONE';
  const canRebase = task.status === 'REVIEW' || task.status === 'RESOLVING_CONFLICTS';
  const canDelete = !canCancel;
  const button = (action: string, label: string, enabled: boolean, className: string, title: string) =>
    `<button class="btn ${className}" data-action="${action}" type="button" ${enabled && !busy ? '' : 'disabled'} title="${escapeHtml(title)}">${label}</button>`;

  return [
    button('run', '▶ Run', canRun, 'primary', canRun ? 'Run this ready task' : 'Only ready tasks can start'),
    button('approve', '✓ Approve', canApprove, 'success', canApprove ? 'Merge this reviewed diff' : 'Only REVIEW tasks can be approved'),
    button('reject', '✕ Reject', canReject, 'danger', canReject ? 'Send this task back with feedback' : 'Only REVIEW or needs-human tasks can be rejected'),
    button('retry', '↺ Retry', canRetry, 'ghost', canRetry ? 'Retry this task' : 'Only failed or completed tasks can be retried'),
    button('rebase', '⇄ Rebase', canRebase, 'ghost', canRebase ? 'Rebase this task workspace onto updated master' : 'Only review tasks can be rebased'),
    button('cancel', '■ Cancel', canCancel, 'danger', canCancel ? 'Stop this agent and release the task' : 'Only running tasks can be cancelled'),
    button('recover', '↻ Recover', canRecover, 'ghost', canRecover ? 'Release this stuck task back to ready' : 'Only running tasks can be recovered'),
    button('delete', '🗑 Delete', canDelete, 'ghost', canDelete ? 'Delete this task' : 'Running tasks cannot be deleted'),
    button('refresh', 'Refresh', true, 'ghost', 'Refresh from the daemon'),
  ].join('');
}

function tabButton(tab: Tab): string {
  const active = state.tab === tab ? ' active' : '';
  return `<button class="tab${active}" data-tab="${tab}" type="button" role="tab" aria-selected="${state.tab === tab}">${TAB_LABEL[tab]}</button>`;
}

function renderInspector(): void {
  const task = selectedTask();
  if (!task) {
    const rawId = (state.selectedId ?? '').toLowerCase();
    const profileKey = CREW_MEMBERS.map((m) => m.id).find((k) => rawId.includes(k));
    const profile = profileKey ? CREW_PROFILES[profileKey] : null;

    if (profile && profileKey) {
      if (!state.crewConfig) {
        void api.crew().then((c) => {
          state.crewConfig = c;
          renderInspector();
        }).catch(() => {});
      }
      if (!state.skillsList) {
        void api.skills().then((s) => {
          state.skillsList = s.skills;
          renderInspector();
        }).catch(() => {});
      }
      if (!state.availableModels) {
        void api.models().then((m) => {
          state.availableModels = m;
          renderInspector();
        }).catch(() => {});
      }

      const crewEntry = state.crewConfig?.crew.find((c) => c.id === profileKey);
      const activeProvider = crewEntry?.provider ?? state.crewConfig?.default_provider ?? 'opencode';
      const activeModel = crewEntry?.model ?? state.crewConfig?.default_model ?? '(default)';
      const isCustom = crewEntry?.is_custom ?? false;
      const providerModels = (state.availableModels
        ? state.availableModels[activeProvider as 'opencode' | 'antigravity']
        : []) || [];
      const currentSelectedModel = crewEntry?.model ?? '';
      const isCustomModel = Boolean(currentSelectedModel && !providerModels.includes(currentSelectedModel));

      // Find all tasks assigned to this crew member
      const assigned = state.tasks.filter((t) => {
        if (t.crew_member) return t.crew_member.toLowerCase().includes(profileKey);
        return guessCrewMember(t) === profileKey;
      });

      let tasksHtml = '';
      if (assigned.length > 0) {
        tasksHtml = `<div class="dossier-section">
          <div class="crew-tasks-title">
            <span>Assigned Tasks</span>
            <span class="crew-pill">${assigned.length} task${assigned.length > 1 ? 's' : ''}</span>
          </div>
          <div style="display: flex; flex-direction: column; gap: 6px;">
            ${assigned.map((t) => `
              <div class="crew-task-item" data-task-id="${escapeHtml(t.id)}">
                <span style="font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escapeHtml(t.title)}</span>
                <span class="status-pill ${statusClass(t.status)}">${statusLabel(t.status)}</span>
              </div>
            `).join('')}
          </div>
        </div>`;
      }

      inspectorEl.innerHTML = `<div class="inspector-head">
        <div class="eyebrow">STRAW HAT PIRATES · BOUNTY ${escapeHtml(profile.bounty)}</div>
        <h2>${profile.emoji} ${escapeHtml(profile.name)}</h2>
        <div class="inspector-state">
          <span class="crew-pill">${profile.emoji} ${escapeHtml(profile.role)}</span>
          ${isCustom ? '<span class="crew-custom-badge">⚡ Custom Model Active</span>' : '<span class="status-pill status-todo">DEFAULT MODEL</span>'}
        </div>
      </div>
      <div class="character-dossier">
        <blockquote class="character-quote">“${escapeHtml(profile.quote)}”</blockquote>

        <!-- In-Browser Model & Provider Switcher -->
        <div class="crew-model-card">
          <h4>
            <span>Model & Provider</span>
            ${isCustom ? '<span class="crew-custom-badge">CUSTOM</span>' : ''}
          </h4>
          <div class="crew-field">
            <label for="crew-provider-select">Provider</label>
            <select id="crew-provider-select">
              <option value="opencode"${activeProvider === 'opencode' ? ' selected' : ''}>OpenCode (opencode)</option>
              <option value="antigravity"${activeProvider === 'antigravity' ? ' selected' : ''}>Antigravity (agy)</option>
            </select>
          </div>
          <div class="crew-field">
            <label for="crew-model-select">Model ID</label>
            <select id="crew-model-select">
              <option value="">(Default for ${activeProvider === 'antigravity' ? 'Antigravity' : 'OpenCode'})</option>
              ${providerModels.map((m) => `<option value="${escapeHtml(m)}"${m === currentSelectedModel ? ' selected' : ''}>${escapeHtml(m)}</option>`).join('')}
              ${isCustomModel ? `<option value="${escapeHtml(currentSelectedModel)}" selected>${escapeHtml(currentSelectedModel)} (custom)</option>` : ''}
              <option value="__custom__">+ Custom model ID…</option>
            </select>
            <input id="crew-model-custom-input" style="display: none; margin-top: 6px;" placeholder="e.g. custom-model-id" value="${escapeHtml(currentSelectedModel)}" />
            <small style="color: var(--muted); font-size: 10px;">Active: <code>${escapeHtml(activeModel)}</code></small>
          </div>
          <div class="crew-card-actions">
            <button id="crew-save-btn" class="btn primary" type="button">Save Model</button>
            <button id="crew-reset-btn" class="btn ghost" type="button"${isCustom ? '' : ' disabled'}>Reset</button>
          </div>
        </div>

        <!-- Skills Card -->
        <div class="crew-model-card">
          <h4>
            <span>Installed Skills (${(crewEntry?.skills ?? []).length})</span>
          </h4>
          <div style="display: flex; flex-wrap: wrap; gap: 4px; margin-bottom: 8px;">
            ${(crewEntry?.skills ?? []).length === 0 ? '<span style="color: var(--muted); font-size: 11px; font-style: italic;">No specific skills installed yet.</span>' : (crewEntry?.skills ?? []).map((sk) => {
              const skillObj = state.skillsList?.find((s) => s.id.toLowerCase() === sk.toLowerCase());
              return `<span class="crew-pill" style="display: inline-flex; align-items: center; gap: 4px; padding: 2px 6px; font-size: 11px;">
                ⚡ ${escapeHtml(skillObj?.name ?? sk)}
                <button class="crew-remove-skill-btn" data-skill="${escapeHtml(sk)}" style="background: none; border: none; cursor: pointer; color: var(--fg); font-weight: bold; padding: 0 2px;">✕</button>
              </span>`;
            }).join('')}
          </div>
          <div style="display: flex; gap: 6px;">
            <select id="crew-skill-select" style="flex: 1; padding: 4px; font-size: 11px; background: var(--bg); color: var(--fg); border: 1px solid var(--border);">
              <option value="">-- Choose skill to install --</option>
              ${(state.skillsList ?? [])
                .filter((s) => !(crewEntry?.skills ?? []).some((k) => k.toLowerCase() === s.id.toLowerCase()))
                .map((s) => `<option value="${escapeHtml(s.id)}">[${escapeHtml(s.source)}] ${escapeHtml(s.name)}</option>`)
                .join('')}
            </select>
            <button id="crew-add-skill-btn" class="btn primary" style="padding: 4px 8px; font-size: 11px;" type="button">+ Install</button>
          </div>
        </div>

        <div class="dossier-section">
          <h3>Specialty & Role</h3>
          <p>${escapeHtml(profile.specialty)}</p>
        </div>
        <div class="dossier-section">
          <h3>Description</h3>
          <p>${escapeHtml(profile.description)}</p>
        </div>

        ${tasksHtml}

        <div class="dossier-actions">
          <button id="dossier-plan-btn" class="btn primary" type="button">Assign Task to ${escapeHtml(profile.name.split(' ').pop()!)}</button>
        </div>
      </div>`;

      document.getElementById('crew-provider-select')?.addEventListener('change', (e) => {
        const newProvider = (e.target as HTMLSelectElement).value;
        const modelSelect = document.getElementById('crew-model-select') as HTMLSelectElement | null;
        const customInput = document.getElementById('crew-model-custom-input') as HTMLInputElement | null;
        if (!modelSelect) return;
        const models = (state.availableModels ? state.availableModels[newProvider as 'opencode' | 'antigravity'] : []) || [];
        modelSelect.innerHTML = `
          <option value="">(Default for ${newProvider === 'antigravity' ? 'Antigravity' : 'OpenCode'})</option>
          ${models.map((m) => `<option value="${escapeHtml(m)}">${escapeHtml(m)}</option>`).join('')}
          <option value="__custom__">+ Custom model ID…</option>
        `;
        if (models.length > 0) {
          modelSelect.value = models[0];
        } else {
          modelSelect.value = '';
        }
        if (customInput) customInput.style.display = 'none';
      });

      document.getElementById('crew-model-select')?.addEventListener('change', (e) => {
        const val = (e.target as HTMLSelectElement).value;
        const customInput = document.getElementById('crew-model-custom-input') as HTMLInputElement | null;
        if (customInput) {
          if (val === '__custom__') {
            customInput.style.display = 'block';
            customInput.focus();
          } else {
            customInput.style.display = 'none';
          }
        }
      });

      document.getElementById('crew-save-btn')?.addEventListener('click', async () => {
        const provider = (document.getElementById('crew-provider-select') as HTMLSelectElement)?.value;
        const modelSelect = document.getElementById('crew-model-select') as HTMLSelectElement | null;
        const customInput = document.getElementById('crew-model-custom-input') as HTMLInputElement | null;
        let model = modelSelect?.value.trim() ?? '';
        if (model === '__custom__') {
          model = customInput?.value.trim() ?? '';
          if (!model) {
            showToast('Please enter a custom model id.', 'error');
            return;
          }
        }
        try {
          showToast(`Updating model for ${profile.name}...`);
          await api.updateCrew(profileKey, { provider, model: model || undefined });
          state.crewConfig = await api.crew();
          showToast(`Updated ${profile.name} to ${model || 'default'}!`);
          renderInspector();
        } catch (err) {
          showToast(`Failed to update model: ${errorMessage(err)}`, 'error');
        }
      });

      document.getElementById('crew-reset-btn')?.addEventListener('click', async () => {
        try {
          showToast(`Resetting ${profile.name} to default...`);
          await api.updateCrew(profileKey, { reset: true });
          state.crewConfig = await api.crew();
          showToast(`Reset ${profile.name} back to default.`);
          renderInspector();
        } catch (err) {
          showToast(`Failed to reset: ${errorMessage(err)}`, 'error');
        }
      });

      document.getElementById('crew-add-skill-btn')?.addEventListener('click', async () => {
        const select = document.getElementById('crew-skill-select') as HTMLSelectElement | null;
        const skillId = select?.value;
        if (!skillId) {
          showToast('Please select a skill to install.', 'error');
          return;
        }
        try {
          showToast(`Installing skill ${skillId} to ${profile.name}...`);
          await api.addCrewSkill(profileKey, skillId);
          state.crewConfig = await api.crew();
          showToast(`Installed ${skillId} to ${profile.name}!`);
          renderInspector();
        } catch (err) {
          showToast(`Failed to install skill: ${errorMessage(err)}`, 'error');
        }
      });

      inspectorEl.querySelectorAll('.crew-remove-skill-btn').forEach((btn) => {
        btn.addEventListener('click', async (e) => {
          e.stopPropagation();
          const skillId = (btn as HTMLElement).dataset.skill;
          if (!skillId) return;
          try {
            showToast(`Removing skill ${skillId} from ${profile.name}...`);
            await api.removeCrewSkill(profileKey, skillId);
            state.crewConfig = await api.crew();
            showToast(`Removed skill ${skillId}.`);
            renderInspector();
          } catch (err) {
            showToast(`Failed to remove skill: ${errorMessage(err)}`, 'error');
          }
        });
      });

      inspectorEl.querySelectorAll('.crew-task-item').forEach((item) => {
        item.addEventListener('click', () => {
          const taskId = (item as HTMLElement).dataset.taskId;
          if (taskId) selectTask(taskId);
        });
      });

      document.getElementById('dossier-plan-btn')?.addEventListener('click', () => {
        promptEl.value = `[${profile.name.split(' ').pop()}] `;
        promptEl.focus();
      });
      return;
    }

    inspectorEl.innerHTML = `<div class="inspector-empty">
      <span class="empty-mark">🏴‍☠️</span>
      <h2>Straw Hat Crew Office</h2>
      <p>Click any pirate desk in the office or a task in the rail to inspect their dossier, output, logs, diff and controls.</p>
    </div>`;
    return;
  }

  const logs = mergedLogs(task);
  let content = '';
  if (state.tab === 'output') {
    content = `<div class="markdown">${renderMarkdown(task.agent_output)}</div>`;
  } else if (state.tab === 'logs') {
    content = `<div class="content-bar"><span>${logs.length} lines</span><span class="${state.connection === 'live' ? 'live-label' : 'muted'}">${state.connection === 'live' ? '● live' : 'snapshot'}</span></div>
      <pre class="logs" id="logs-pre">${escapeHtml(logs.join('\n')) || '<span class="muted">No logs yet.</span>'}</pre>`;
  } else if (state.tab === 'diff') {
    content = `<div class="content-bar"><span>Unified diff</span><span class="muted">captured at review</span></div>${renderDiff(task.diff)}`;
  } else {
    content = `<dl class="meta-grid">
      <dt>Status</dt><dd><span class="status-pill ${statusClass(task.status)}">${statusLabel(task.status)}</span></dd>
      <dt>Crew</dt><dd>${task.crew_member ? `<button class="btn ghost" style="padding: 2px 7px; font-size: 11px;" id="task-crew-link" type="button">${crewEmoji(task.crew_member)} ${escapeHtml(task.crew_member)} · ⚙️ Model</button>` : '—'}</dd>
      <dt>Task id</dt><dd class="mono">${escapeHtml(task.id)}</dd>
      <dt>Retries</dt><dd>${task.retry_count}</dd>
      <dt>Created</dt><dd>${escapeHtml(formatDate(task.created_at))}</dd>
      <dt>Updated</dt><dd>${escapeHtml(formatDate(task.updated_at))}</dd>
    </dl>
    <h3>Description</h3>
    <div class="description">${escapeHtml(task.description).replace(/\n/g, '<br>') || '<span class="muted">No description.</span>'}</div>`;
  }

  inspectorEl.innerHTML = `<div class="inspector-head">
      <div class="eyebrow">Task ${escapeHtml(task.id.slice(-10))}</div>
      <h2>${escapeHtml(task.title)}</h2>
      <div class="inspector-state">
        <span class="status-pill ${statusClass(task.status)}">${statusLabel(task.status)}</span>
        ${task.crew_member ? `<span class="crew-pill">${crewEmoji(task.crew_member)} ${escapeHtml(task.crew_member)}</span>` : ''}
        ${state.busy ? `<span class="busy-label">${escapeHtml(state.busy)}…</span>` : ''}
      </div>
    </div>
    <div class="actions">${actionButtons(task)}</div>
    <div class="tabs" role="tablist">${(Object.keys(TAB_LABEL) as Tab[]).map(tabButton).join('')}</div>
    <div class="tab-body">${content}</div>`;

  if (task.crew_member) {
    document.getElementById('task-crew-link')?.addEventListener('click', () => {
      selectDesk(task.crew_member!);
    });
  }

  if (state.tab === 'logs') {
    const pre = document.getElementById('logs-pre');
    if (pre) pre.scrollTop = pre.scrollHeight;
  }
}

function renderOfficeStatus(): void {
  const agents = tracker.getAll();
  const working = agents.filter((agent) => agent.visualState === 'working').length;
  const waiting = agents.filter((agent) => agent.visualState === 'waiting').length;
  const ready = agents.length - working - waiting;
  officeStatusEl.innerHTML = `<b>${agents.length}</b> crew aboard <span class="dot working"></span>${working} active
    <span class="dot waiting"></span>${waiting} review <span class="dot idle"></span>${ready} ready`;
  emptyEl.hidden = true; // Thousand Sunny crew is always on deck!

  const liveTask = state.tasks.find((task) => ACTIVE_STATUSES.includes(task.status)) ?? selectedTask();
  const lines = liveTask ? mergedLogs(liveTask) : [];
  liveTitleEl.textContent = liveTask ? liveTask.title : 'Live transcript';
  liveMetaEl.textContent = liveTask ? `${lines.length} lines · ${statusLabel(liveTask.status)}` : '';
  liveLogsEl.textContent = liveTask
    ? lines.length > 0
      ? lines.slice(-24).join('\n')
      : 'No log lines yet.'
    : 'Select a task to see its live transcript.';
  liveLogsEl.scrollTop = liveLogsEl.scrollHeight;
}

function renderError(): void {
  errorEl.hidden = !state.error;
  errorTextEl.textContent = state.error ?? '';
}

function render(): void {
  renderHeader();
  renderRail();
  renderAttachments();
  renderInspector();
  renderOfficeStatus();
  renderError();
}

function scheduleRender(): void {
  if (renderQueued) return;
  renderQueued = true;
  requestAnimationFrame(() => {
    renderQueued = false;
    render();
  });
}

function showToast(message: string, kind: 'info' | 'error' = 'info'): void {
  toastEl.textContent = message;
  toastEl.className = `toast visible ${kind}`;
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toastEl.className = 'toast';
    toastTimer = null;
  }, 4500);
}

function applyVersion(info: VersionResponse): void {
  if (zouVersionEl) {
    zouVersionEl.textContent = `v${info.current_version}`;
    zouVersionEl.title = `Zunesha Version ${info.current_version}`;
  }
  if (docsLink && info.docs_url) {
    docsLink.href = info.docs_url;
  }
  if (githubLink && info.repo_url) {
    githubLink.href = info.repo_url;
  }
  if (updateBtn) {
    if (info.update_available) {
      updateBtn.hidden = false;
      updateBtn.textContent = `⚡ Update to v${info.latest_version}`;
      updateBtn.title = `Sequential update available: ${info.pending_updates.join(' → ')}. Click to update and restart server.`;
      updateBtn.onclick = () => void handleUpdate(info);
    } else {
      updateBtn.hidden = true;
    }
  }
}

async function handleUpdate(info: VersionResponse): Promise<void> {
  const steps = info.pending_updates && info.pending_updates.length > 0
    ? `\nSteps: ${info.pending_updates.join(' → ')}`
    : '';
  if (!confirm(`Update sequentially to v${info.latest_version} and restart daemon?${steps}`)) {
    return;
  }
  if (updateBtn) {
    updateBtn.disabled = true;
    updateBtn.textContent = 'Updating…';
  }
  showToast('Updating step-by-step & restarting server…', 'info');
  try {
    await api.applyUpdate();
    const poll = window.setInterval(async () => {
      try {
        const res = await api.status();
        if (res.initialized) {
          window.clearInterval(poll);
          window.location.reload();
        }
      } catch {
        // Server restarting in background
      }
    }, 1500);
  } catch (err) {
    if (updateBtn) {
      updateBtn.disabled = false;
      updateBtn.textContent = `⚡ Update to v${info.latest_version}`;
    }
    showToast(`Update failed: ${errorMessage(err)}`, 'error');
  }
}

// ── state application ──────────────────────────────────────────────────────

function syncOffice(): void {
  // Always maintain all 5 Straw Hat crew members at their Thousand Sunny stations!
  const agents: PlaygroundAgent[] = CREW_MEMBERS.map((member) => {
    // Find all tasks assigned or matched to this Straw Hat
    const assignedTasks = state.tasks.filter((t) => {
      if (t.crew_member) return t.crew_member.toLowerCase().includes(member.id);
      return guessCrewMember(t) === member.id;
    });

    const activeTask = assignedTasks.find((t) => ACTIVE_STATUSES.includes(t.status))
      ?? assignedTasks.find((t) => t.status === 'REVIEW')
      ?? assignedTasks.find((t) => t.status === 'TODO')
      ?? assignedTasks[0];

    if (activeTask) {
      return {
        id: member.id,
        name: member.name,
        title: activeTask.title,
        status: activeTask.status,
        state: STATUS_VISUAL[activeTask.status] ?? 'idle',
        crew_member: member.id,
        queueCount: assignedTasks.length,
        updated_at: activeTask.updated_at,
        retry_count: activeTask.retry_count,
      };
    }

    return {
      id: member.id,
      name: member.name,
      title: `${member.role} · On Standby`,
      status: 'TODO',
      state: 'idle',
      crew_member: member.id,
      queueCount: 0,
      updated_at: '',
      retry_count: 0,
    };
  });

  tracker.setSnapshot(agents);
  renderer.setProject(state.project?.name ?? 'THOUSAND SUNNY');

  // Highlight active or selected crew member on canvas
  const task = selectedTask();
  if (task) {
    const member = task.crew_member ? task.crew_member.toLowerCase() : guessCrewMember(task);
    renderer.setSelected(member);
  } else {
    renderer.setSelected(state.selectedId);
  }
}

function triggerCrewDialogue(
  task: Task,
  prevStatus: TaskStatus | undefined,
  nextStatus: TaskStatus,
): void {
  if (prevStatus === nextStatus) return;

  const specialistKey = task.crew_member
    ? task.crew_member.toLowerCase()
    : guessCrewMember(task);

  const titleShort = task.title.length > 28 ? `${task.title.slice(0, 26)}…` : task.title;

  switch (nextStatus) {
    case 'IN_PROGRESS':
    case 'RESOLVING_CONFLICTS': {
      soundManager.playCannon();
      soundManager.playHandoff();
      const quotes: Record<string, string> = {
        zoro: `⚔️ Cutting debt: "${titleShort}"`,
        sanji: `🍳 Cooking UI: "${titleShort}"`,
        usopp: `🎯 Hunting bugs: "${titleShort}"`,
        luffy: `🍖 Let's build: "${titleShort}"`,
        nami: `🧭 Charting: "${titleShort}"`,
      };
      const quote = quotes[specialistKey] ?? `🏴‍☠️ Starting: "${titleShort}"`;
      tracker.setBubble(specialistKey, quote, 'info', 14_000);
      break;
    }
    case 'EVALUATING': {
      soundManager.playHandoff();
      tracker.setBubble('usopp', `🎯 Testing & QA: "${titleShort}"`, 'info', 14_000);
      break;
    }
    case 'REVIEW': {
      soundManager.playChime();
      tracker.setBubble('luffy', `🏴‍☠️ Completed "${titleShort}"! Ready for review!`, 'success', 16_000);
      break;
    }
    case 'DONE': {
      soundManager.playChime();
      tracker.setBubble('luffy', `✨ Integrated: "${titleShort}"! Kanpai! 🍻`, 'success', 16_000);
      break;
    }
    case 'FAILED_NEEDS_HUMAN': {
      soundManager.playAlert();
      tracker.setBubble(specialistKey, `⚠️ Stuck on "${titleShort}"! Need Captain!`, 'error', 18_000);
      break;
    }
    case 'TODO': {
      if (prevStatus === 'BACKLOG') {
        tracker.setBubble('nami', `🗺️ Unblocked "${titleShort}" for ${specialistKey.toUpperCase()}!`, 'info', 12_000);
      }
      break;
    }
  }
}

function applySnapshot(snapshot: TasksResponse): void {
  state.project = snapshot.project;
  state.tasks = snapshot.tasks;
  state.dependencies = snapshot.dependencies;
  state.counts = snapshot.counts;
  state.liveLogs.clear();
  if (!state.selectedId || !snapshot.tasks.some((task) => task.id === state.selectedId)) {
    state.selectedId = pickDefaultTask(snapshot.tasks);
  }
  syncOffice();
  render();
}

function applyTask(task: Task): void {
  const previous = state.tasks.find((candidate) => candidate.id === task.id);
  const prevStatus = previous?.status;

  const index = state.tasks.findIndex((candidate) => candidate.id === task.id);
  if (index === -1) state.tasks = [...state.tasks, task];
  else state.tasks = state.tasks.map((candidate) => (candidate.id === task.id ? task : candidate));

  if (prevStatus && prevStatus !== task.status) {
    triggerCrewDialogue(task, prevStatus, task.status);
  }

  const live = state.liveLogs.get(task.id);
  if (live) {
    const persisted = lineCount(task.worker_logs);
    for (const seq of [...live.keys()]) {
      if (seq < persisted) live.delete(seq);
    }
  }

  if (!state.selectedId) state.selectedId = task.id;
  syncOffice();
  render();
}

function applyLog(event: LogEvent): void {
  let live = state.liveLogs.get(event.task_id);
  if (!live) {
    live = new Map();
    state.liveLogs.set(event.task_id, live);
  }
  live.set(event.seq, event);

  const task = state.tasks.find((candidate) => candidate.id === event.task_id);
  if (task) {
    const persisted = lineCount(task.worker_logs);
    for (const seq of [...live.keys()]) {
      if (seq < persisted) live.delete(seq);
    }
  }
  const activeTask = state.tasks.find((candidate) => ACTIVE_STATUSES.includes(candidate.status));
  if ((state.selectedId === event.task_id && state.tab === 'logs') || activeTask?.id === event.task_id) {
    scheduleRender();
  }
}

function selectTask(id: string | null): void {
  state.selectedId = id;
  const task = selectedTask();
  state.tab = task?.agent_output ? 'output' : 'logs';
  renderer.setSelected(id);
  render();
}

// ── actions ────────────────────────────────────────────────────────────────

async function refresh(): Promise<void> {
  applySnapshot(await api.tasks());
}

async function withBusy(label: string, action: () => Promise<void>): Promise<void> {
  if (state.busy) return;
  state.busy = label;
  state.error = null;
  render();
  try {
    await action();
  } catch (error) {
    state.error = errorMessage(error);
    renderError();
  } finally {
    state.busy = null;
    render();
  }
}

async function planFeature(): Promise<void> {
  const prompt = promptEl.value.trim();
  if (!prompt) {
    promptEl.focus();
    showToast('Describe a feature first.', 'error');
    return;
  }
  await withBusy('planning', async () => {
    tracker.setBubble('nami', '🗺️ Plotting mission coordinates & blueprint...', 'info', 10_000);
    const res = await api.plan(prompt, toPayload(state.planAttachments));
    promptEl.value = '';
    clearAttachments('plan');
    await refresh();
    soundManager.playChime();
    tracker.setBubble(
      'nami',
      `🗺️ Mission chartered! Created ${res.task_ids.length} task${res.task_ids.length === 1 ? '' : 's'}!`,
      'success',
      12_000,
    );
    showToast('Plan created. Review the new tasks, then run them.');
  });
}

async function runAll(): Promise<void> {
  const ready = state.tasks.filter((task) => task.status === 'TODO').length;
  if (ready === 0) return;
  if (!window.confirm(`Run ${ready} ready task${ready === 1 ? '' : 's'}?`)) return;
  await withBusy('starting', async () => {
    soundManager.playCannon();
    const result = await api.runAll();
    await refresh();
    showToast(`${result.started} task${result.started === 1 ? '' : 's'} started.`);
  });
}

async function runOne(id: string): Promise<void> {
  if (!window.confirm('Run this task now?')) return;
  await withBusy('starting', async () => {
    soundManager.playCannon();
    await api.runOne(id);
    await refresh();
    showToast('Task started.');
  });
}

async function approve(id: string): Promise<void> {
  if (!window.confirm('Approve this task and merge its diff into the working tree?')) return;
  await withBusy('approving', async () => {
    const result = await api.approve(id);
    await refresh();
    soundManager.playChime();
    if (result.integration.status === 'conflict_resolution_queued') {
      showToast('Merge conflict: an agent is resolving it. Review the result when it returns.');
    } else if (result.integration.status === 'applied') {
      showToast(`Approved. ${result.integration.files?.length ?? 0} file(s) landed.`);
    } else {
      showToast('Approved. There were no file changes to land.');
    }
  });
}

async function cancel(id: string): Promise<void> {
  if (!window.confirm('Stop this agent and release the task back to ready?')) return;
  await withBusy('cancelling', async () => {
    await api.cancel(id);
    await refresh();
    showToast('Cancellation requested; the task will return to ready.');
  });
}

async function recover(id: string): Promise<void> {
  if (!window.confirm('Release this stuck task back to ready?')) return;
  await withBusy('recovering', async () => {
    const result = await api.recover(id);
    await refresh();
    showToast(`${result.released.length} task${result.released.length === 1 ? '' : 's'} released.`);
  });
}

async function retryTask(id: string): Promise<void> {
  if (!window.confirm('Retry this task now?')) return;
  await withBusy('starting', async () => {
    soundManager.playCannon();
    await api.retry(id);
    await refresh();
    showToast('Task resumed and restarted.');
  });
}

async function rebaseTask(id: string): Promise<void> {
  if (!window.confirm('Rebase this task workspace onto updated master?')) return;
  await withBusy('starting', async () => {
    await api.rebase(id);
    await refresh();
    showToast('Workspace rebased successfully.');
  });
}

async function deleteTask(id: string): Promise<void> {
  if (!window.confirm('Delete this task and delete its workspace? This cannot be undone.')) return;
  await withBusy('starting', async () => {
    await api.deleteTask(id);
    if (state.selectedId === id) state.selectedId = null;
    await refresh();
    showToast('Task deleted.');
  });
}

function openReject(task: Task): void {
  rejectTaskId = task.id;
  modalTitleEl.textContent = `Reject “${task.title}”`;
  rejectFeedbackEl.value = '';
  clearAttachments('reject');
  modalEl.hidden = false;
  window.setTimeout(() => rejectFeedbackEl.focus(), 30);
}

function closeReject(): void {
  rejectTaskId = null;
  modalEl.hidden = true;
}

async function submitReject(): Promise<void> {
  const id = rejectTaskId;
  if (!id) return;
  const feedback = rejectFeedbackEl.value.trim();
  const attachments = toPayload(state.rejectAttachments);
  closeReject();
  await withBusy('rejecting', async () => {
    await api.reject(id, feedback, attachments);
    clearAttachments('reject');
    await refresh();
    showToast('Task rejected and returned to ready.');
  });
}

function handleInspectorAction(action: string): void {
  const task = selectedTask();
  if (action === 'refresh') {
    void withBusy('refreshing', refresh);
    return;
  }
  if (!task) return;
  if (action === 'run') void runOne(task.id);
  else if (action === 'approve') void approve(task.id);
  else if (action === 'reject') openReject(task);
  else if (action === 'cancel') void cancel(task.id);
  else if (action === 'recover') void recover(task.id);
  else if (action === 'retry') void retryTask(task.id);
  else if (action === 'rebase') void rebaseTask(task.id);
  else if (action === 'delete') void deleteTask(task.id);
}

// ── events ─────────────────────────────────────────────────────────────────

taskListEl.addEventListener('click', (event) => {
  const target = (event.target as HTMLElement).closest<HTMLElement>('[data-task-id]');
  if (target) selectTask(target.dataset.taskId ?? null);
});

inspectorEl.addEventListener('click', (event) => {
  const target = (event.target as HTMLElement).closest<HTMLElement>('[data-action], [data-tab]');
  if (!target) return;
  const tab = target.dataset.tab as Tab | undefined;
  if (tab) {
    state.tab = tab;
    renderInspector();
    return;
  }
  const action = target.dataset.action;
  if (action) handleInspectorAction(action);
});

searchEl.addEventListener('input', () => {
  state.filter = searchEl.value;
  renderRail();
});

planBtn.addEventListener('click', () => void planFeature());
runAllBtn.addEventListener('click', () => void runAll());

attachBtn.addEventListener('click', () => attachInput.click());
rejectAttachBtn.addEventListener('click', () => rejectAttachInput.click());
attachInput.addEventListener('change', () => {
  if (attachInput.files?.length) void addAttachments('plan', attachInput.files);
  attachInput.value = '';
});
rejectAttachInput.addEventListener('change', () => {
  if (rejectAttachInput.files?.length) void addAttachments('reject', rejectAttachInput.files);
  rejectAttachInput.value = '';
});
for (const list of [attachList, rejectAttachList]) {
  list.addEventListener('click', (event) => {
    const target = (event.target as HTMLElement).closest<HTMLElement>('[data-remove-attachment]');
    if (!target) return;
    const kind = target.dataset.removeAttachment as 'plan' | 'reject';
    const index = Number(target.dataset.index);
    if (kind === 'plan') {
      state.planAttachments = state.planAttachments.filter((_, i) => i !== index);
      state.planAttachmentErrors = [];
    } else {
      state.rejectAttachments = state.rejectAttachments.filter((_, i) => i !== index);
      state.rejectAttachmentErrors = [];
    }
    renderAttachments();
  });
}

promptEl.addEventListener('paste', (event) => {
  if (event.clipboardData?.files.length) {
    event.preventDefault();
    void addAttachments('plan', event.clipboardData.files);
  }
});
promptEl.addEventListener('dragover', (event) => {
  if (event.dataTransfer?.types.includes('Files')) event.preventDefault();
});
promptEl.addEventListener('drop', (event) => {
  if (!event.dataTransfer?.files.length) return;
  event.preventDefault();
  void addAttachments('plan', event.dataTransfer.files);
});
rejectFeedbackEl.addEventListener('paste', (event) => {
  if (event.clipboardData?.files.length) {
    event.preventDefault();
    void addAttachments('reject', event.clipboardData.files);
  }
});
rejectFeedbackEl.addEventListener('dragover', (event) => {
  if (event.dataTransfer?.types.includes('Files')) event.preventDefault();
});
rejectFeedbackEl.addEventListener('drop', (event) => {
  if (!event.dataTransfer?.files.length) return;
  event.preventDefault();
  void addAttachments('reject', event.dataTransfer.files);
});

promptEl.addEventListener('keydown', (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
    event.preventDefault();
    void planFeature();
  }
});

function selectDesk(crewId: string): void {
  const memberKey = ['luffy', 'zoro', 'nami', 'usopp', 'sanji'].find((k) => crewId.toLowerCase().includes(k));
  if (!memberKey) {
    selectTask(crewId);
    return;
  }

  const assigned = state.tasks.filter((t) => {
    if (t.crew_member) return t.crew_member.toLowerCase().includes(memberKey);
    return guessCrewMember(t) === memberKey;
  });

  const activeTask = assigned.find((t) => ACTIVE_STATUSES.includes(t.status))
    ?? assigned.find((t) => t.status === 'REVIEW')
    ?? assigned.find((t) => t.status === 'TODO')
    ?? assigned[0];

  // If already inspecting this active task, or no active task exists, show the pirate's dossier
  if (state.selectedId === activeTask?.id || !activeTask) {
    selectTask(memberKey);
  } else {
    selectTask(activeTask.id);
  }
}

canvas.addEventListener('click', (event) => {
  const rect = canvas.getBoundingClientRect();
  const x = ((event.clientX - rect.left) / rect.width) * 360;
  const y = ((event.clientY - rect.top) / rect.height) * 280;
  const id = renderer.agentAt(x, y);
  if (id) selectDesk(id);
});

modalCancelEl.addEventListener('click', closeReject);
modalSubmitEl.addEventListener('click', () => void submitReject());
modalEl.addEventListener('click', (event) => {
  if (event.target === modalEl) closeReject();
});

document.getElementById('error-close')?.addEventListener('click', () => {
  state.error = null;
  renderError();
});

document.getElementById('empty-plan')?.addEventListener('click', () => promptEl.focus());

document.addEventListener('keydown', (event) => {
  const element = event.target as HTMLElement | null;
  if (element && (element.tagName === 'INPUT' || element.tagName === 'TEXTAREA')) return;

  if (event.key === 'Escape') {
    closeReject();
    return;
  }
  if (event.key === 'p') {
    event.preventDefault();
    promptEl.focus();
  } else if (event.key === 'g') {
    event.preventDefault();
    void withBusy('refreshing', refresh);
  } else if (event.key === 'j' || event.key === 'k') {
    event.preventDefault();
    const tasks = visibleTasks();
    if (tasks.length === 0) return;
    const index = tasks.findIndex((task) => task.id === state.selectedId);
    const next = event.key === 'j' ? Math.min(index + 1, tasks.length - 1) : Math.max(index - 1, 0);
    selectTask(tasks[next]?.id ?? null);
  } else if (event.key === 'o' || event.key === 'l' || event.key === 'd' || event.key === 'i') {
    const tab: Tab = event.key === 'o' ? 'output' : event.key === 'l' ? 'logs' : event.key === 'd' ? 'diff' : 'task';
    state.tab = tab;
    renderInspector();
  } else if (event.key === 'r') {
    event.preventDefault();
    void runAll();
  } else if (event.key === 'a' && selectedTask()) {
    event.preventDefault();
    void approve(selectedTask()!.id);
  } else if (event.key === 'x') {
    const task = selectedTask();
    if (task && (task.status === 'REVIEW' || task.status === 'FAILED_NEEDS_HUMAN')) {
      event.preventDefault();
      openReject(task);
    }
  } else if (event.key === 'C') {
    const task = selectedTask();
    if (task && (task.status === 'IN_PROGRESS' || task.status === 'EVALUATING' || task.status === 'RESOLVING_CONFLICTS')) {
      event.preventDefault();
      void cancel(task.id);
    }
  }
});

// ── live connection ────────────────────────────────────────────────────────

function handleEvent(event: ApiEvent): void {
  if (event.type === 'snapshot') {
    applySnapshot(event);
    return;
  }
  if (event.type === 'task_updated') {
    applyTask(event.task);
    return;
  }
  if (event.type === 'log') applyLog(event);
}

function connect(): void {
  state.connection = 'connecting';
  renderHeader();
  const next = eventSocket({
    onOpen: () => {
      state.connection = 'live';
      renderHeader();
    },
    onEvent: handleEvent,
    onError: (error) => console.warn('[playground] websocket event failed:', error),
    onClose: () => {
      state.connection = 'offline';
      renderHeader();
      if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
      reconnectTimer = window.setTimeout(() => {
        reconnectTimer = null;
        connect();
      }, 2000);
    },
  });
  socket = next;
}

async function loadInitial(): Promise<void> {
  try {
    const [snapshot, crew, skills, ver, models] = await Promise.all([
      api.tasks(),
      api.crew().catch(() => null),
      api.skills().catch(() => null),
      api.version().catch(() => null),
      api.models().catch(() => null),
    ]);
    if (crew) state.crewConfig = crew;
    if (skills) state.skillsList = skills.skills;
    if (ver) applyVersion(ver);
    if (models) state.availableModels = models;
    applySnapshot(snapshot);
  } catch (error) {
    state.connection = 'offline';
    state.error = `Could not reach the daemon: ${errorMessage(error)}`;
    render();
  }
}

const sfxToggleEl = document.getElementById('sfx-toggle') as HTMLButtonElement | null;
function updateSfxButton(): void {
  if (!sfxToggleEl) return;
  const on = soundManager.isEnabled();
  sfxToggleEl.textContent = on ? '🔊 SFX' : '🔇 SFX';
  sfxToggleEl.classList.toggle('active', on);
  sfxToggleEl.title = on
    ? 'Audio enabled: 8-bit chiptune SFX and ocean waves playing'
    : 'Audio muted: Click to enable 8-bit sound effects & ocean waves';
}
sfxToggleEl?.addEventListener('click', () => {
  soundManager.toggle();
  updateSfxButton();
});
updateSfxButton();

renderer.start();
void loadInitial();
connect();

window.addEventListener('beforeunload', () => {
  socket?.close();
  if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
});
