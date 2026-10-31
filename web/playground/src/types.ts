export type AgentId = string;  // a Zunesha task id

export type VisualState =
  | 'idle'
  | 'working'
  | 'reading'
  | 'delivering'
  | 'error'
  | 'sleeping'
  | 'waiting'
  | 'done';

export type ActivityType = 'coffee' | 'water' | 'stretch' | 'whiteboard' | 'meeting';

export interface AgentActivity {
  type: ActivityType;
  startedAt: number;      // ms timestamp
  walkMs: number;         // 3000ms — time to walk there and back
  stayMs: number;         // 8000–14000ms — time at activity
  targetX: number;        // logical canvas X destination
  targetY: number;        // logical canvas Y destination
  deskX: number;          // agent's desk X (to return to)
  deskY: number;          // agent's desk Y
}

export interface AgentAppearance {
  hairColor: string;      // hex, e.g. '#2A1A0A'
  outfitColor: string;    // hex — overrides the character's shirt/body color
  skinTone: string;       // hex, e.g. '#D4A870'
  accessory: 'none' | 'glasses' | 'headset' | 'hat' | 'cap' | 'strawhat' | 'earrings' | 'logpose' | 'goggles' | 'cigarette';
}

export interface CronJobState {
  name: string;
  status: string;
  summary: string;
  durationMs: number;
  nextRunAtMs: number | null;
  lastRunAtMs: number | null;
  consecutiveErrors: number;
}

export interface HardwareState {
  cpu: { loadAvg: number[]; count: number; pct: number; model?: string };
  mem: { totalBytes: number; usedBytes: number; pct: number };
  gpu: { name: string; utilPct: number; memUsedMB: number; memTotalMB: number; tempC: number } | null;
  gpuExtra?: { powerW: number; clockGrMhz: number; clockMemMhz: number } | null;
  ollama: { models: string[]; busy: boolean };
  ollamaDetail?: Array<{ name: string; sizeVramMB: number }>;
  disk?: { totalBytes: number; usedBytes: number; availBytes: number } | null;
}

export interface Bubble {
  text: string;
  type: 'info' | 'error' | 'success';
  expiresAt: number;
}

export interface AgentState {
  id: AgentId;
  name: string;
  title: string;
  status: string;
  crew_member?: string;
  retryCount: number;
  emoji: string;
  color: string;
  secondaryColor: string;
  visualState: VisualState;
  lastActivity: number;
  cronJobs: CronJobState[];
  bubble: Bubble | null;
  tokenUsage: { input: number; output: number };
  activity: AgentActivity | null;
}
