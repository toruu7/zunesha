// Zunesha task tracker for the pixel office.
//
// The office renderer only needs a small, stable view of the board. This
// tracker accepts task projections from the control room (it ignores logs,
// diffs, and output), keeps task order stable, and turns lifecycle changes
// into short speech bubbles so a review/error is visible at a glance.

import { agentColor, agentEmoji, computeLayout, type AgentLayout } from './layout';
import type { AgentState, Bubble, VisualState } from './types';

export interface PlaygroundAgent {
  id: string;
  name: string;
  title: string;
  status: string;
  state: string;
  updated_at: string;
  retry_count: number;
}

const VALID_STATES: VisualState[] = [
  'idle',
  'working',
  'reading',
  'delivering',
  'error',
  'sleeping',
  'waiting',
  'done',
];

function visualState(value: string): VisualState {
  return VALID_STATES.includes(value as VisualState) ? (value as VisualState) : 'idle';
}

function timestamp(value: string): number {
  const parsed = Date.parse(value.replace(' ', 'T') + 'Z');
  return Number.isFinite(parsed) ? parsed : Date.now();
}

export class AgentTracker {
  private agents = new Map<string, AgentState>();
  private order: string[] = [];

  setSnapshot(agents: PlaygroundAgent[]): void {
    const next = new Map<string, AgentState>();
    const order: string[] = [];
    for (const agent of agents) {
      const state = this.toState(agent, this.agents.get(agent.id));
      next.set(agent.id, state);
      order.push(agent.id);
    }
    this.agents = next;
    this.order = order;
  }

  update(agent: PlaygroundAgent): void {
    const previous = this.agents.get(agent.id);
    const next = this.toState(agent, previous);
    if (!this.agents.has(agent.id)) this.order.push(agent.id);
    this.agents.set(agent.id, next);
  }

  tick(): void {
    const now = Date.now();
    for (const agent of this.agents.values()) {
      if (agent.bubble && now > agent.bubble.expiresAt) agent.bubble = null;
    }
  }

  getAll(): AgentState[] {
    return this.order.map((id) => this.agents.get(id)).filter((agent): agent is AgentState => !!agent);
  }

  getLayouts(): AgentLayout[] {
    return computeLayout(
      this.getAll().map((agent) => ({
        id: agent.id,
        name: agent.title || agent.name,
      })),
    );
  }

  getAgent(id: string): AgentState | undefined {
    return this.agents.get(id);
  }

  private toState(agent: PlaygroundAgent, previous?: AgentState): AgentState {
    const state = visualState(agent.state);
    const { color, secondary } = agentColor(agent.id);
    const changed = previous && previous.visualState !== state;
    let bubble = previous?.bubble ?? null;
    if (changed) {
      bubble = bubbleFor(state, agent.title);
    }
    return {
      id: agent.id,
      name: agent.name,
      title: agent.title,
      status: agent.status,
      retryCount: agent.retry_count,
      emoji: agentEmoji(agent.id),
      color,
      secondaryColor: secondary,
      visualState: state,
      lastActivity: timestamp(agent.updated_at),
      cronJobs: [],
      bubble,
      tokenUsage: { input: 0, output: 0 },
      activity: null,
    };
  }
}

function bubbleFor(state: VisualState, title: string): Bubble | null {
  const text =
    state === 'waiting'
      ? `Review: ${title}`
      : state === 'error'
        ? `Needs human: ${title}`
        : state === 'done'
          ? `Done: ${title}`
          : null;
  if (!text) return null;
  return {
    text: text.length > 90 ? `${text.slice(0, 87)}...` : text,
    type: state === 'error' ? 'error' : state === 'done' ? 'success' : 'info',
    expiresAt: Date.now() + 20_000,
  };
}
