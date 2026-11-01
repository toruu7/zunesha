// Canvas 2D renderer — draws the office scene with animated agents

import { AgentTracker } from './agents';

import {
  drawCharacter, drawDesk,
  drawHelm, drawJollyRogerFlag, drawTreasureChest, drawRumBarrels,
  drawShipCannon, drawSunnyMast, drawMikanTrees, drawAnchorAndRope,
  SPRITE_SIZE,
} from './sprites';
import { drawBubble } from './bubbles';

const CANVAS_W = 360;
const CANVAS_H = 280;
const S = SPRITE_SIZE.scale;
const WORK_FRAME_RATE = 0.25;

export class Renderer {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private tracker: AgentTracker;
  private frame = 0;
  private frameTimer = 0;
  private lastTime = 0;
  private bgCache: HTMLCanvasElement | null = null;
  private lastBgFrame = -1;
  private projectName = 'ZOU OFFICE';
  private selectedId: string | null = null;
  private running = false;

  constructor(canvas: HTMLCanvasElement, tracker: AgentTracker) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d')!;
    this.tracker = tracker;

    canvas.width  = CANVAS_W * S;
    canvas.height = CANVAS_H * S;
    canvas.style.width  = `${CANVAS_W * S}px`;
    canvas.style.height = `${CANVAS_H * S}px`;
    canvas.style.imageRendering = 'pixelated';
    this.ctx.imageSmoothingEnabled = false;
  }

  setProject(name: string): void {
    if (!name.trim()) return;
    this.projectName = name.trim().toUpperCase();
    this.lastBgFrame = -1;
  }

  setSelected(id: string | null): void {
    this.selectedId = id;
  }

  /** Return the task id whose desk/character contains logical point (x, y). */
  agentAt(x: number, y: number): string | null {
    const layouts = this.tracker.getLayouts();
    for (let i = layouts.length - 1; i >= 0; i--) {
      const layout = layouts[i];
      if (x >= layout.deskX - 3 && x <= layout.deskX + 33 && y >= layout.deskY - 4 && y <= layout.deskY + 44) {
        return layout.id;
      }
    }
    return null;
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.lastTime = performance.now();
    this.loop(this.lastTime);
  }

  stop(): void {
    this.running = false;
  }

  private loop = (time: number): void => {
    if (!this.running) return;
    const dt = Math.min((time - this.lastTime) / 1000, 0.1);
    this.lastTime = time;
    this.frameTimer += dt;
    if (this.frameTimer >= WORK_FRAME_RATE) {
      this.frame++;
      this.frameTimer = 0;
    }
    if (this.frame % 20 === 0) this.tracker.tick();
    this.render();
    requestAnimationFrame(this.loop);
  };

  private render(): void {
    const ctx = this.ctx;
    const w = this.canvas.width;
    const h = this.canvas.height;

    if (this.frame !== this.lastBgFrame) {
      this.bgCache = this.renderBackground(this.frame);
      this.lastBgFrame = this.frame;
    }
    ctx.drawImage(this.bgCache!, 0, 0);

    const layouts = this.tracker.getLayouts();
    const agents  = this.tracker.getAll();
    const agentMap = new Map(agents.map(a => [a.id, a]));

    for (const layout of layouts) {
      const agent = agentMap.get(layout.id);
      if (!agent) continue;

      const crewOrId = agent.crew_member ?? layout.id;
      drawDesk(ctx, layout.deskX, layout.deskY, layout.color, layout.label, crewOrId);
      const animFrame = this.getAnimFrame(agent.visualState);
      drawCharacter(ctx, layout.deskX, layout.deskY, layout.color, layout.secondaryColor,
        agent.visualState, animFrame, crewOrId);
      this.drawStatusDot(ctx, layout.deskX, layout.deskY, agent.visualState);

      if (layout.id === this.selectedId) {
        ctx.save();
        ctx.strokeStyle = '#FBBF24'; // Pirate Gold
        ctx.globalAlpha = 0.55 + 0.25 * Math.sin(this.frame * 0.4);
        ctx.lineWidth = 1.5;
        ctx.setLineDash([4, 3]);
        ctx.strokeRect(
          (layout.deskX - 4) * S,
          (layout.deskY - 5) * S,
          40 * S,
          47 * S,
        );
        ctx.restore();
      }

      if (agent.bubble) {
        drawBubble(ctx, layout.deskX, layout.deskY, agent.bubble.text, agent.bubble.type);
      }
    }

    this.drawStatusBar(ctx, w, h);
  }

  // ── animation ─────────────────────────────────────────────────────────────

  private getAnimFrame(state: string): number {
    const f = this.frame;
    switch (state) {
      case 'working':   return f % 4;
      case 'reading':   return Math.floor(f / 3) % 2;
      case 'sleeping':  return Math.floor(f / 5) % 3;
      case 'error':     return f % 2;
      case 'delivering':return Math.floor(f / 2) % 2;
      case 'waiting':   return Math.floor(f / 3) % 2;
      case 'done':      return Math.floor(f / 2) % 2;
      default:          return Math.floor(f / 8) % 2;
    }
  }

  private drawStatusDot(ctx: CanvasRenderingContext2D, x: number, y: number, state: string): void {
    const colors: Record<string, string> = {
      working: '#00DD00', reading: '#00BBDD', delivering: '#FFD700',
      error: '#FF3333', sleeping: '#666688', idle: '#888888',
      waiting: '#FFB000', done: '#45B7FF',
    };
    const color = colors[state] ?? '#888';
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc((x + 16) * S + 2 * S, (y + 2) * S, 2 * S, 0, Math.PI * 2);
    ctx.fill();

    if (state === 'working' || state === 'error' || state === 'waiting') {
      ctx.strokeStyle = color;
      ctx.globalAlpha = 0.3 + 0.3 * Math.sin(this.frame * 0.5);
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc((x + 16) * S + 2 * S, (y + 2) * S, 3 * S, 0, Math.PI * 2);
      ctx.stroke();
      ctx.globalAlpha = 1;
    }
  }

  // ── background ────────────────────────────────────────────────────────────

  private renderBackground(frame: number): HTMLCanvasElement {
    const bg = document.createElement('canvas');
    bg.width  = this.canvas.width;
    bg.height = this.canvas.height;
    const ctx = bg.getContext('2d')!;
    ctx.imageSmoothingEnabled = false;
    const w = bg.width, h = bg.height;

    // ── 1. SKY & CLOUDS (y: 0 to 14 logical px) ──
    ctx.fillStyle = '#38BDF8'; // Sky blue
    ctx.fillRect(0, 0, w, 14 * S);
    // Fluffy pixel clouds
    ctx.fillStyle = 'rgba(255, 255, 255, 0.85)';
    ctx.fillRect(24 * S, 3 * S, 28 * S, 5 * S);
    ctx.fillRect(28 * S, 1 * S, 20 * S, 3 * S);
    ctx.fillRect(240 * S, 4 * S, 32 * S, 5 * S);
    ctx.fillRect(246 * S, 2 * S, 20 * S, 3 * S);

    // ── 2. OPEN SEA WITH ROLLING WAVES (y: 12 to 24 logical px) ──
    ctx.fillStyle = '#0284C7'; // Deep ocean
    ctx.fillRect(0, 13 * S, w, 11 * S);
    // Wave foam crests (animated with frame)
    ctx.fillStyle = '#BAE6FD';
    for (let i = 0; i < 12; i++) {
      const waveX = ((i * 32 + (frame * 2)) % 360) * S;
      ctx.fillRect(waveX, 16 * S + (i % 2) * 2 * S, 14 * S, 2 * S);
      ctx.fillRect(waveX + 2 * S, 15 * S + (i % 2) * 2 * S, 10 * S, 1 * S);
    }

    // ── 3. THOUSAND SUNNY SHIP RAILING (y: 20 to 25 logical px) ──
    ctx.fillStyle = '#78350F'; // Dark teak railing
    ctx.fillRect(0, 21 * S, w, 4 * S);
    ctx.fillStyle = '#B45309'; // Golden oak handrail
    ctx.fillRect(0, 20 * S, w, 2 * S);
    ctx.fillStyle = '#F59E0B'; // Brass top rail
    ctx.fillRect(0, 20 * S, w, 1 * S);
    // Railing posts/balusters every 20 logical px
    ctx.fillStyle = '#92400E';
    for (let p = 0; p < 18; p++) {
      ctx.fillRect((p * 20 + 8) * S, 21 * S, 3 * S, 4 * S);
    }

    // ── 4. THOUSAND SUNNY WOODEN DECK (y: 24 to bottom) ──
    ctx.fillStyle = '#B45309'; // Golden teak ship deck
    ctx.fillRect(0, 24 * S, w, h - 24 * S);
    for (let row = 0; row < 28; row++) {
      const py = 25 * S + row * 9 * S;
      ctx.fillStyle = row % 2 === 0 ? '#92400E' : '#B45309';
      ctx.fillRect(0, py, w, 8 * S);
      // Pitch caulking between planks
      ctx.fillStyle = '#451A03';
      ctx.fillRect(0, py + 8 * S, w, 1 * S);
    }
    // Vertical wood plank seams
    ctx.fillStyle = '#451A03';
    ctx.globalAlpha = 0.25;
    for (let col = 0; col < 12; col++) {
      ctx.fillRect(col * 30 * S + 10 * S, 25 * S, 1 * S, h);
    }
    ctx.globalAlpha = 1;

    // ── 5. THOUSAND SUNNY BRIDGE & PROPS ──
    // Center mast & Straw Hat Jolly Roger
    drawSunnyMast(ctx, 160, 5);
    drawJollyRogerFlag(ctx, 120, 3, frame);

    // Sunny Helm (ship's wheel) at the bridge
    drawHelm(ctx, 164, 18);

    // Port side (left) naval cannon, rum barrels & treasure chest
    drawShipCannon(ctx, 6, 26);
    drawRumBarrels(ctx, 6, 110);
    drawTreasureChest(ctx, 10, 210);

    // Starboard side (right) Nami's Mikan trees, anchor, and sake barrels
    drawMikanTrees(ctx, 332, 26);
    drawAnchorAndRope(ctx, 332, 110);
    drawRumBarrels(ctx, 332, 210);

    // Sunny Nameplate Banner
    ctx.fillStyle = '#7F1D1D'; // Pirate Crimson
    ctx.fillRect(125 * S, 3 * S, 110 * S, 10 * S);
    ctx.fillStyle = '#F59E0B'; // Gold trim
    ctx.fillRect(125 * S, 3 * S, 110 * S, 1 * S);
    ctx.fillRect(125 * S, 12 * S, 110 * S, 1 * S);
    ctx.fillStyle = '#FEF08A'; // Yellow lettering
    ctx.font = `bold ${3.8 * S}px monospace`;
    ctx.textAlign = 'center';
    ctx.fillText(`THOUSAND SUNNY`, 180 * S, 10 * S);
    ctx.textAlign = 'start';

    return bg;
  }

  // ── status bar ────────────────────────────────────────────────────────────

  private drawStatusBar(ctx: CanvasRenderingContext2D, w: number, h: number): void {
    const barH = 24 * S, barY = h - barH;
    ctx.fillStyle = 'rgba(20, 20, 30, 0.92)';
    ctx.fillRect(0, barY, w, barH);
    ctx.fillStyle = '#4A90D9';
    ctx.fillRect(0, barY, w, 1 * S);

    const now = new Date();
    const timeStr = now.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
    ctx.fillStyle = '#AAAACC';
    ctx.font = `${3.5 * S}px monospace`;
    ctx.fillText(timeStr, 4 * S, barY + 8 * S);

    const agents = this.tracker.getAll();
    const working = agents.filter((agent) => agent.visualState === 'working').length;
    const waiting = agents.filter((agent) => agent.visualState === 'waiting').length;
    const sleeping = agents.filter((agent) => agent.visualState === 'sleeping').length;
    ctx.fillStyle = '#8888AA';
    ctx.font = `${2.5 * S}px monospace`;
    ctx.fillText(
      `Straw Hat Pirates · ${agents.length} crew · ${working} working · ${waiting} waiting · ${sleeping} sleeping`,
      28 * S,
      barY + 8 * S,
    );

    // Status dots: one per agent, in stable order.
    const summary = agents.map((agent) =>
      agent.visualState === 'error'
        ? '\u{1F534}'
        : agent.visualState === 'working'
          ? '\u{1F7E2}'
          : agent.visualState === 'waiting'
            ? '\u{1F7E1}'
            : agent.visualState === 'done'
              ? '\u{1F535}'
              : agent.visualState === 'sleeping'
                ? '\u{1F7E4}'
                : '\u{26AA}',
    ).join(' ');
    ctx.font = `${4 * S}px monospace`;
    ctx.textAlign = 'right';
    ctx.fillText(summary, w - 4 * S, barY + 12 * S);
    ctx.textAlign = 'start';

    ctx.fillStyle = '#666677';
    ctx.font = `${3 * S}px monospace`;
    ctx.textAlign = 'right';
    ctx.fillText(`${agents.length} crew aboard`, w - 4 * S, barY + 20 * S);
    ctx.textAlign = 'start';
  }
}
