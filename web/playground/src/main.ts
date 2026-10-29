// Zunesha office — an interactive control room for the local daemon.
//
// The canvas is the office view; the rail and inspector are ordinary DOM so
// task selection, plan/run/approve/reject/recover, live logs, diffs and final
// Markdown output all work without leaving the page. The page talks to the
// same REST + `/ws` API as the dashboard.

import './style.css';
import { AgentTracker, type PlaygroundAgent } from './agents';
import { Renderer } from './renderer';
import {
  api,
  eventSocket,
  type ApiEvent,
  type DependencyEdge,
  type LogEvent,
  type ProjectInfo,
  type StatusCount,
  type Task,
  type TaskStatus,
  type TasksResponse,
} from './api';

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
};

const tracker = new AgentTracker();
const canvas = document.getElementById('office') as HTMLCanvasElement;
const renderer = new Renderer(canvas, tracker);

const projectNameEl = document.getElementById('project-name') as HTMLElement;
const projectRootEl = document.getElementById('project-root') as HTMLElement;
const countsEl = document.getElementById('counts') as HTMLElement;
const connectionEl = document.getElementById('connection') as HTMLElement;
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

function taskToAgent(task: Task): PlaygroundAgent {
  return {
    id: task.id,
    name: task.title,
    title: task.title,
    status: task.status,
    state: STATUS_VISUAL[task.status] ?? 'idle',
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
        return `<button class="task-item${selected}" data-task-id="${escapeHtml(task.id)}" type="button">
          <span class="task-dot ${visual}" aria-hidden="true"></span>
          <span class="task-copy">
            <span class="task-title">${escapeHtml(task.title)}</span>
            <span class="task-meta"><span class="task-state ${statusClass(task.status)}">${statusLabel(task.status)}</span><span class="task-id">${escapeHtml(task.id.slice(-7))}</span></span>
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

function actionButtons(task: Task): string {
  const busy = state.busy !== null;
  const canRun = task.status === 'TODO';
  const canApprove = task.status === 'REVIEW';
  const canReject = task.status === 'REVIEW' || task.status === 'FAILED_NEEDS_HUMAN';
  const canRecover = task.status === 'IN_PROGRESS' || task.status === 'EVALUATING' || task.status === 'RESOLVING_CONFLICTS';
  const button = (action: string, label: string, enabled: boolean, className: string, title: string) =>
    `<button class="btn ${className}" data-action="${action}" type="button" ${enabled && !busy ? '' : 'disabled'} title="${escapeHtml(title)}">${label}</button>`;

  return [
    button('run', '▶ Run', canRun, 'primary', canRun ? 'Run this ready task' : 'Only ready tasks can start'),
    button('approve', '✓ Approve', canApprove, 'success', canApprove ? 'Merge this reviewed diff' : 'Only REVIEW tasks can be approved'),
    button('reject', '✕ Reject', canReject, 'danger', canReject ? 'Send this task back with feedback' : 'Only REVIEW or needs-human tasks can be rejected'),
    button('recover', '↻ Recover', canRecover, 'ghost', canRecover ? 'Release this stuck task back to ready' : 'Only running tasks can be recovered'),
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
    inspectorEl.innerHTML = `<div class="inspector-empty">
      <span class="empty-mark">⌁</span>
      <h2>Select an agent</h2>
      <p>Click a desk in the office or a task in the rail to inspect its output, logs, diff and controls.</p>
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
        ${state.busy ? `<span class="busy-label">${escapeHtml(state.busy)}…</span>` : ''}
      </div>
    </div>
    <div class="actions">${actionButtons(task)}</div>
    <div class="tabs" role="tablist">${(Object.keys(TAB_LABEL) as Tab[]).map(tabButton).join('')}</div>
    <div class="tab-body">${content}</div>`;

  if (state.tab === 'logs') {
    const pre = document.getElementById('logs-pre');
    if (pre) pre.scrollTop = pre.scrollHeight;
  }
}

function renderOfficeStatus(): void {
  const agents = tracker.getAll();
  const working = agents.filter((agent) => agent.visualState === 'working').length;
  const waiting = agents.filter((agent) => agent.visualState === 'waiting').length;
  const sleeping = agents.filter((agent) => agent.visualState === 'sleeping').length;
  officeStatusEl.innerHTML = `<b>${agents.length}</b> agents <span class="dot working"></span>${working} working
    <span class="dot waiting"></span>${waiting} waiting <span class="dot sleeping"></span>${sleeping} sleeping`;
  emptyEl.hidden = agents.length > 0;

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

// ── state application ──────────────────────────────────────────────────────

function syncOffice(): void {
  tracker.setSnapshot(state.tasks.map(taskToAgent));
  renderer.setProject(state.project?.name ?? 'ZOU');
  renderer.setSelected(state.selectedId);
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
  const index = state.tasks.findIndex((candidate) => candidate.id === task.id);
  if (index === -1) state.tasks = [...state.tasks, task];
  else state.tasks = state.tasks.map((candidate) => (candidate.id === task.id ? task : candidate));

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
    await api.plan(prompt);
    promptEl.value = '';
    await refresh();
    showToast('Plan created. Review the new tasks, then run them.');
  });
}

async function runAll(): Promise<void> {
  const ready = state.tasks.filter((task) => task.status === 'TODO').length;
  if (ready === 0) return;
  if (!window.confirm(`Run ${ready} ready task${ready === 1 ? '' : 's'}?`)) return;
  await withBusy('starting', async () => {
    const result = await api.runAll();
    await refresh();
    showToast(`${result.started} task${result.started === 1 ? '' : 's'} started.`);
  });
}

async function runOne(id: string): Promise<void> {
  if (!window.confirm('Run this task now?')) return;
  await withBusy('starting', async () => {
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
    if (result.integration.status === 'conflict_resolution_queued') {
      showToast('Merge conflict: an agent is resolving it. Review the result when it returns.');
    } else if (result.integration.status === 'applied') {
      showToast(`Approved. ${result.integration.files?.length ?? 0} file(s) landed.`);
    } else {
      showToast('Approved. There were no file changes to land.');
    }
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

function openReject(task: Task): void {
  rejectTaskId = task.id;
  modalTitleEl.textContent = `Reject “${task.title}”`;
  rejectFeedbackEl.value = '';
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
  closeReject();
  await withBusy('rejecting', async () => {
    await api.reject(id, feedback);
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
  else if (action === 'recover') void recover(task.id);
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
promptEl.addEventListener('keydown', (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
    event.preventDefault();
    void planFeature();
  }
});

canvas.addEventListener('click', (event) => {
  const rect = canvas.getBoundingClientRect();
  const x = ((event.clientX - rect.left) / rect.width) * 360;
  const y = ((event.clientY - rect.top) / rect.height) * 280;
  const id = renderer.agentAt(x, y);
  if (id) selectTask(id);
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
    applySnapshot(await api.tasks());
  } catch (error) {
    state.connection = 'offline';
    state.error = `Could not reach the daemon: ${errorMessage(error)}`;
    render();
  }
}

renderer.start();
void loadInitial();
connect();

window.addEventListener('beforeunload', () => {
  socket?.close();
  if (reconnectTimer !== null) window.clearTimeout(reconnectTimer);
});
