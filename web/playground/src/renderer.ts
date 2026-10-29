// Canvas 2D renderer — draws the office scene with animated agents

import { AgentTracker } from './agents';

import {
  drawCharacter, drawDesk,
  drawServerRack, drawWhiteboard, drawCorkboard, drawBookshelf,
  drawFilingCabinet, drawCoffeeStation, drawPlant, drawMeetingTable,
  drawWaterCooler, drawCeilingLight, drawClock,
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

      drawDesk(ctx, layout.deskX, layout.deskY, layout.color, layout.label, layout.id);
      const animFrame = this.getAnimFrame(agent.visualState);
      drawCharacter(ctx, layout.deskX, layout.deskY, layout.color, layout.secondaryColor,
        agent.visualState, animFrame, layout.id);
      this.drawStatusDot(ctx, layout.deskX, layout.deskY, agent.visualState);

      if (layout.id === this.selectedId) {
        ctx.save();
        ctx.strokeStyle = '#7DD3FC';
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

    // Floor — wood planks
    ctx.fillStyle = '#2A2018';
    ctx.fillRect(0, 20 * S, w, h - 20 * S);
    for (let row = 0; row < 30; row++) {
      const py = 20 * S + row * 9 * S;
      ctx.fillStyle = row % 2 === 0 ? '#2E2418' : '#32291C';
      ctx.fillRect(0, py, w, 9 * S);
      ctx.fillStyle = '#1A140E';
      ctx.fillRect(0, py + 8 * S, w, 1 * S);
    }
    ctx.fillStyle = '#1A140E';
    ctx.globalAlpha = 0.4;
    for (let col = 0; col < 10; col++) {
      ctx.fillRect(col * 36 * S, 20 * S, 1 * S, h);
      ctx.fillRect(col * 36 * S + 18 * S, 20 * S, 1 * S, h);
    }
    ctx.globalAlpha = 1;

    // Top wall
    ctx.fillStyle = '#323248';
    ctx.fillRect(0, 0, w, 22 * S);
    ctx.fillStyle = '#3A3A50';
    ctx.fillRect(0, 18 * S, w, 4 * S);
    ctx.fillStyle = '#4A4A68';
    ctx.fillRect(0, 18 * S, w, 1 * S);
    ctx.fillStyle = '#282840';
    ctx.fillRect(0, 21 * S, w, 1 * S);
    ctx.fillStyle = '#28284A';
    ctx.fillRect(0, 8 * S, w, 2 * S);
    ctx.fillStyle = '#2E2E44';
    ctx.fillRect(0, 22 * S, 2 * S, h - 22 * S);
    ctx.fillRect(w - 2 * S, 22 * S, 2 * S, h - 22 * S);

    // Ceiling lights
    drawCeilingLight(ctx, 120, 5, true, frame);
    drawCeilingLight(ctx, 240, 5, true, frame);

    // Clock
    drawClock(ctx, 172, 3);

    // HQ sign
    ctx.fillStyle = '#5A5A80';
    ctx.fillRect(56 * S, 4 * S, 70 * S, 10 * S);
    ctx.fillStyle = '#3A3A60';
    ctx.fillRect(56 * S, 4 * S, 70 * S, 1 * S);
    ctx.fillStyle = '#9999CC';
    ctx.font = `bold ${4 * S}px monospace`;
    ctx.textAlign = 'center';
    ctx.fillText(`${this.projectName.slice(0, 12)} HQ`, 91 * S, 11 * S);
    ctx.textAlign = 'start';

    // Window
    ctx.fillStyle = '#1A2A3A';
    ctx.fillRect(200 * S, 4 * S, 28 * S, 14 * S);
    ctx.fillStyle = '#2A4A6A';
    ctx.fillRect(201 * S, 5 * S, 26 * S, 12 * S);
    ctx.fillStyle = '#4A5060';
    ctx.fillRect(213 * S, 5 * S, 1 * S, 12 * S);
    ctx.fillRect(201 * S, 10 * S, 26 * S, 1 * S);
    ctx.fillStyle = 'rgba(180,220,255,0.15)';
    ctx.fillRect(202 * S, 6 * S, 10 * S, 4 * S);

    // Left side furniture
    drawServerRack(ctx, 4, 22, frame);
    drawWhiteboard(ctx, 20, 22);
    drawPlant(ctx, 3, 143, false);

    // Right side furniture
    drawCorkboard(ctx, 288, 22);
    drawFilingCabinet(ctx, 344, 22);

    // Center corridor
    drawMeetingTable(ctx, 148, 115);
    drawCoffeeStation(ctx, 158, 218, frame);
    drawWaterCooler(ctx, 316, 220);

    // Bibliotecario zone
    drawBookshelf(ctx, 338, 105);

    // Felipe zone
    drawFilingCabinet(ctx, 4, 158);

    // Plants — corners
    drawPlant(ctx, 0, 238, true);
    drawPlant(ctx, 348, 238, true);
    drawPlant(ctx, 0, 44, false);
    drawPlant(ctx, 350, 44, false);

    // Floor mats (computed from current layouts)
    const matColors = ['#4A3A50', '#2A4A38', '#5A3A20', '#3A2A4A',
                       '#5A2A2A', '#2A4A4A', '#4A4A2A', '#2A2A4A',
                       '#4A3A2A', '#2A4A3A', '#3A2A4A', '#3A3A2A'];
    const layouts = this.tracker.getLayouts();
    layouts.forEach((layout, i) => {
      const mc = matColors[i % matColors.length];
      ctx.fillStyle = mc;
      ctx.globalAlpha = 0.35;
      ctx.fillRect((layout.deskX - 6) * S, (layout.deskY + 12) * S, 34 * S, 30 * S);
      ctx.globalAlpha = 1;
      ctx.strokeStyle = mc;
      ctx.lineWidth = 1;
      ctx.globalAlpha = 0.6;
      ctx.strokeRect((layout.deskX - 6) * S, (layout.deskY + 12) * S, 34 * S, 30 * S);
      ctx.globalAlpha = 1;
    });

    ctx.fillStyle = '#1A1A28';
    ctx.fillRect(0, 21 * S, w, 1 * S);

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
      `${agents.length} agents · ${working} working · ${waiting} waiting · ${sleeping} sleeping`,
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
    ctx.fillText(`${agents.length} agents`, w - 4 * S, barY + 20 * S);
    ctx.textAlign = 'start';
  }
}
