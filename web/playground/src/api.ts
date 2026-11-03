// REST + WebSocket access for the interactive office.
//
// The page is served by the daemon, so relative URLs work in production. Vite
// dev proxies `/api` and `/ws` to 127.0.0.1:57804 (see vite.config.mts).

export const STATUSES = [
  'BACKLOG',
  'TODO',
  'IN_PROGRESS',
  'EVALUATING',
  'REVIEW',
  'RESOLVING_CONFLICTS',
  'DONE',
  'FAILED_NEEDS_HUMAN',
] as const;

export type TaskStatus = (typeof STATUSES)[number];

export interface Task {
  id: string;
  title: string;
  description: string;
  status: TaskStatus;
  crew_member?: string;
  retry_count: number;
  worker_logs: string;
  agent_output: string;
  diff: string;
  created_at: string;
  updated_at: string;
}

export interface ProjectInfo {
  name: string;
  root: string;
}

export interface DependencyEdge {
  task_id: string;
  depends_on: string;
}

export interface StatusCount {
  status: TaskStatus;
  count: number;
}

export interface CrewMember {
  id: string;
  name: string;
  role: string;
  provider: string;
  model: string | null;
  is_custom: boolean;
  skills: string[];
}

export interface Skill {
  id: string;
  name: string;
  description: string;
  instructions: string;
  source: 'builtin' | 'project' | 'user';
}

export interface ModelsResponse {
  default_provider: string;
  opencode: string[];
  antigravity: string[];
}

export interface CrewResponse {
  default_provider: string;
  default_model: string | null;
  crew: CrewMember[];
}

export interface VersionResponse {
  current_version: string;
  latest_version: string;
  update_available: boolean;
  pending_updates: string[];
  docs_url: string;
  repo_url: string;
}

export interface UpdateApplyResponse {
  ok: boolean;
  message: string;
  steps: string[];
  target_version: string;
}

export interface TasksResponse {
  project: ProjectInfo;
  tasks: Task[];
  dependencies: DependencyEdge[];
  counts: StatusCount[];
}

export interface AttachmentPayload {
  name: string;
  media_type: string;
  /** Raw base64, without a data: prefix. */
  data: string;
}

export interface LogEvent {
  task_id: string;
  stream: 'stdout' | 'stderr';
  line: string;
  seq: number;
}

export type ApiEvent =
  | ({ type: 'snapshot' } & TasksResponse)
  | { type: 'task_updated'; task: Task }
  | ({ type: 'log' } & LogEvent);

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: 'same-origin',
    ...init,
    headers: { 'content-type': 'application/json' },
  });
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      const body = (await res.json()) as { error?: string };
      if (body?.error) message = body.error;
    } catch {
      // Keep the status text.
    }
    throw new ApiError(res.status, message);
  }
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

function post<T>(path: string, body?: unknown): Promise<T> {
  return request<T>(path, { method: 'POST', body: JSON.stringify(body ?? {}) });
}

export const api = {
  tasks: () => request<TasksResponse>('/api/tasks'),
  status: () => request<{ initialized: boolean }>('/api/status'),
  plan: (prompt: string, attachments: AttachmentPayload[] = []) =>
    post<{ task_ids: string[] }>('/api/plan', { prompt, attachments }),
  runAll: () => post<{ started: number }>('/api/run'),
  runOne: (id: string) => post<{ started: number }>(`/api/tasks/${encodeURIComponent(id)}/run`),
  cancel: (id: string) =>
    post<{ task_id: string; cancelled: boolean }>(
      `/api/tasks/${encodeURIComponent(id)}/cancel`,
    ),
  approve: (id: string) =>
    post<{ task: Task; promoted: string[]; integration: { status: string; files?: string[] } }>(
      `/api/tasks/${encodeURIComponent(id)}/approve`,
    ),
  reject: (id: string, feedback: string, attachments: AttachmentPayload[] = []) =>
    post<Task>(`/api/tasks/${encodeURIComponent(id)}/reject`, { feedback, attachments }),
  retry: (id: string, mode: 'continue' | 'feedback' | 'hard_reset' = 'continue', message?: string) =>
    post<{ task: Task }>(`/api/tasks/${encodeURIComponent(id)}/retry`, { mode, message }),
  rebase: (id: string) => post<{ task: Task }>(`/api/tasks/${encodeURIComponent(id)}/rebase`),
  deleteTask: (id: string) =>
    request<{ deleted: string }>(`/api/tasks/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  recover: (id: string) => post<{ released: string[] }>('/api/recover', { task_id: id }),
  crew: () => request<CrewResponse>('/api/crew'),
  updateCrew: (
    member: string,
    payload: { provider?: string; model?: string; reset?: boolean; skills?: string[] },
  ) =>
    request<CrewMember>(`/api/crew/${encodeURIComponent(member)}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    }),
  skills: () => request<{ skills: Skill[] }>('/api/skills'),
  addCrewSkill: (member: string, skill: string) =>
    post<{ crew_member: string; skill: string; skills: string[] }>(
      `/api/crew/${encodeURIComponent(member)}/skills`,
      { skill },
    ),
  removeCrewSkill: (member: string, skill: string) =>
    request<{ crew_member: string; removed: string; skills: string[] }>(
      `/api/crew/${encodeURIComponent(member)}/skills/${encodeURIComponent(skill)}`,
      { method: 'DELETE' },
    ),
  version: () => request<VersionResponse>('/api/version'),
  applyUpdate: (repo?: string) =>
    post<UpdateApplyResponse>('/api/update', { repo }),
  models: () => request<ModelsResponse>('/api/models'),
};

export interface EventSocketHandlers {
  onEvent: (event: ApiEvent) => void;
  onOpen: () => void;
  onClose: () => void;
  onError: (error: unknown) => void;
}

export function eventSocket(handlers: EventSocketHandlers): WebSocket {
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const socket = new WebSocket(`${protocol}//${location.host}/ws`);
  socket.addEventListener('open', handlers.onOpen);
  socket.addEventListener('close', handlers.onClose);
  socket.addEventListener('message', (message) => {
    try {
      handlers.onEvent(JSON.parse(String(message.data)) as ApiEvent);
    } catch (error) {
      handlers.onError(error);
    }
  });
  socket.addEventListener('error', (error) => {
    handlers.onError(error);
    socket.close();
  });
  return socket;
}
