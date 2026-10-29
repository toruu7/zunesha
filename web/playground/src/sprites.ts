// Procedural pixel art sprite generator
import { VisualState, HardwareState, ActivityType, AgentAppearance } from './types';

const CHAR_W = 16;
const CHAR_H = 24;
export const SCALE = 3;
const s = SCALE;

// ─── CHARACTERS ───────────────────────────────────────────────────────────────

export function drawCharacter(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  color: string,
  secondaryColor: string,
  state: VisualState,
  frame: number,
  agentId?: string,
  appearance?: AgentAppearance,
): void {
  const cx = x * s;
  const cy = y * s;
  ctx.save();

  // Resolve colors — use appearance overrides if provided
  const skinColor  = appearance?.skinTone  ?? '#D4A870';
  const outfitColor = appearance?.outfitColor ?? color;
  const hairColor  = appearance?.hairColor
    ?? (agentId === 'analista' ? '#2A4A2A'
      : agentId === 'bibliotecario' ? '#3A2060'
      : agentId === 'rafael' ? '#A05010'
      : '#2A1A0A');
  const accessory  = appearance?.accessory
    ?? (agentId === 'main' ? 'headset' : agentId === 'analista' ? 'glasses' : 'none');

  let bodyOffsetY = 0;
  let armOffset = 0;
  switch (state) {
    case 'working':
      bodyOffsetY = frame % 2 === 0 ? 0 : -1 * s;
      armOffset = frame % 2 === 0 ? 0 : 2 * s;
      break;
    case 'reading':
      bodyOffsetY = frame % 2 === 0 ? 0 : 1 * s;
      break;
    case 'sleeping':
      bodyOffsetY = 1 * s;
      break;
    case 'error':
      bodyOffsetY = frame % 2 === 0 ? 0 : -2 * s;
      break;
    case 'delivering':
      bodyOffsetY = frame % 2 === 0 ? -1 * s : 0;
      break;
    case 'waiting':
      bodyOffsetY = frame % 2 === 0 ? 0 : 1 * s;
      break;
    case 'done':
      bodyOffsetY = frame % 2 === 0 ? -1 * s : 0;
      break;
  }

  // Shadow
  ctx.fillStyle = 'rgba(0,0,0,0.25)';
  ctx.fillRect(cx + 2 * s, cy + 22 * s, 12 * s, 2 * s);

  const by = cy + bodyOffsetY;

  // Chair back (behind character)
  ctx.fillStyle = '#1A1A2A';
  ctx.fillRect(cx + 3 * s, by + 8 * s, 11 * s, 8 * s);
  ctx.fillStyle = '#252535';
  ctx.fillRect(cx + 4 * s, by + 9 * s, 9 * s, 6 * s);

  // Legs
  ctx.fillStyle = secondaryColor;
  ctx.fillRect(cx + 5 * s, by + 18 * s, 3 * s, 4 * s);
  ctx.fillRect(cx + 9 * s, by + 18 * s, 3 * s, 4 * s);

  // Torso
  ctx.fillStyle = outfitColor;
  ctx.fillRect(cx + 4 * s, by + 10 * s, 9 * s, 8 * s);

  // Collar/shirt detail
  ctx.fillStyle = lighten(outfitColor, 30);
  ctx.fillRect(cx + 7 * s, by + 10 * s, 3 * s, 2 * s);

  // Arms
  ctx.fillStyle = outfitColor;
  if (state === 'working') {
    ctx.fillRect(cx + 2 * s, by + 12 * s + armOffset, 2 * s, 5 * s);
    ctx.fillRect(cx + 13 * s, by + 12 * s - armOffset, 2 * s, 5 * s);
  } else if (state === 'delivering') {
    ctx.fillRect(cx + 2 * s, by + 11 * s, 2 * s, 6 * s);
    ctx.fillRect(cx + 13 * s, by + 6 * s + bodyOffsetY, 2 * s, 6 * s);
    // Envelope
    ctx.fillStyle = '#F5F0CC';
    ctx.fillRect(cx + 13 * s, by + 4 * s + bodyOffsetY, 4 * s, 3 * s);
    ctx.fillStyle = '#CC9900';
    ctx.fillRect(cx + 13 * s, by + 4 * s + bodyOffsetY, 4 * s, 1 * s);
  } else {
    ctx.fillRect(cx + 2 * s, by + 11 * s, 2 * s, 6 * s);
    ctx.fillRect(cx + 13 * s, by + 11 * s, 2 * s, 6 * s);
  }

  // Hands
  ctx.fillStyle = skinColor;
  if (state === 'working') {
    ctx.fillRect(cx + 2 * s, by + 17 * s + armOffset, 2 * s, 2 * s);
    ctx.fillRect(cx + 13 * s, by + 17 * s - armOffset, 2 * s, 2 * s);
  } else {
    ctx.fillRect(cx + 2 * s, by + 17 * s, 2 * s, 2 * s);
    ctx.fillRect(cx + 13 * s, by + 17 * s, 2 * s, 2 * s);
  }

  // Head
  ctx.fillStyle = skinColor;
  ctx.fillRect(cx + 5 * s, by + 2 * s, 7 * s, 8 * s);

  // Neck
  ctx.fillStyle = darken(skinColor, 20);
  ctx.fillRect(cx + 7 * s, by + 10 * s, 3 * s, 1 * s);

  // Hair
  ctx.fillStyle = hairColor;
  ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 3 * s);
  ctx.fillRect(cx + 4 * s, by + 2 * s, 1 * s, 5 * s);
  ctx.fillRect(cx + 12 * s, by + 2 * s, 1 * s, 5 * s);

  // Accessories
  if (accessory === 'headset') {
    ctx.fillStyle = '#444';
    ctx.fillRect(cx + 3 * s, by + 2 * s, 11 * s, 1 * s);
    ctx.fillRect(cx + 3 * s, by + 2 * s, 1 * s, 3 * s);
    ctx.fillStyle = '#888';
    ctx.fillRect(cx + 3 * s, by + 4 * s, 2 * s, 2 * s);
  } else if (accessory === 'glasses') {
    ctx.fillStyle = '#556';
    ctx.fillRect(cx + 5 * s, by + 5 * s, 3 * s, 2 * s);
    ctx.fillRect(cx + 9 * s, by + 5 * s, 3 * s, 2 * s);
    ctx.fillRect(cx + 8 * s, by + 6 * s, 1 * s, 1 * s);
  } else if (accessory === 'hat') {
    // Hat band
    ctx.fillStyle = '#1A1A1A';
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 3 * s);
    // Hat brim
    ctx.fillRect(cx + 3 * s, by + 4 * s, 11 * s, 1 * s);
    // Hat top
    ctx.fillRect(cx + 5 * s, by - 2 * s, 7 * s, 3 * s);
  } else if (accessory === 'cap') {
    // Cap body (rounded top)
    ctx.fillStyle = '#333';
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 3 * s);
    ctx.fillRect(cx + 5 * s, by - 1 * s, 7 * s, 2 * s);
    // Brim
    ctx.fillRect(cx + 4 * s, by + 4 * s, 9 * s, 1 * s);
    ctx.fillRect(cx + 13 * s, by + 4 * s, 2 * s, 1 * s);
  }

  // Eyes
  if (state === 'sleeping') {
    ctx.fillStyle = hairColor;
    ctx.fillRect(cx + 6 * s, by + 5 * s, 2 * s, 1 * s);
    ctx.fillRect(cx + 10 * s, by + 5 * s, 2 * s, 1 * s);
  } else {
    ctx.fillStyle = '#111';
    ctx.fillRect(cx + 6 * s, by + 5 * s, 2 * s, 2 * s);
    ctx.fillRect(cx + 10 * s, by + 5 * s, 2 * s, 2 * s);
    ctx.fillStyle = '#FFF';
    ctx.fillRect(cx + 7 * s, by + 5 * s, 1 * s, 1 * s);
    ctx.fillRect(cx + 11 * s, by + 5 * s, 1 * s, 1 * s);
  }

  // Mouth
  ctx.fillStyle = '#8A5030';
  if (state === 'error') {
    ctx.fillRect(cx + 7 * s, by + 8 * s, 3 * s, 1 * s);
  } else if (state === 'delivering' || state === 'working') {
    ctx.fillRect(cx + 7 * s, by + 8 * s, 4 * s, 1 * s);
    ctx.fillRect(cx + 6 * s, by + 7 * s, 1 * s, 1 * s);
    ctx.fillRect(cx + 11 * s, by + 7 * s, 1 * s, 1 * s);
  } else {
    ctx.fillRect(cx + 7 * s, by + 8 * s, 3 * s, 1 * s);
  }

  // State accessories
  if (state === 'sleeping') {
    drawZzz(ctx, cx + 14 * s, by - 2 * s + (frame % 3) * -2 * s);
  }
  if (state === 'error') {
    if (frame % 2 === 0) {
      ctx.fillStyle = 'rgba(255,50,50,0.25)';
      ctx.fillRect(cx, cy, CHAR_W * s, CHAR_H * s);
    }
    ctx.fillStyle = '#FF4444';
    ctx.font = `bold ${5 * s}px monospace`;
    ctx.fillText('!', cx + 15 * s, by + 3 * s);
  }

  ctx.restore();
}

export function drawActivityIndicator(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  type: ActivityType,
  _frame: number,
): void {
  void _frame;
  const ix = x * s;
  const iy = (y - 8) * s;
  ctx.save();

  // Background bubble
  ctx.fillStyle = 'rgba(20,20,40,0.7)';
  ctx.fillRect(ix - 1 * s, iy - 1 * s, 8 * s, 8 * s);

  switch (type) {
    case 'coffee': {
      // Mug body
      ctx.fillStyle = '#6A3A10';
      ctx.fillRect(ix + 1 * s, iy + 2 * s, 5 * s, 4 * s);
      // Mug handle
      ctx.fillStyle = '#6A3A10';
      ctx.fillRect(ix + 6 * s, iy + 3 * s, 1 * s, 2 * s);
      // Steam
      ctx.fillStyle = 'rgba(200,200,200,0.7)';
      ctx.fillRect(ix + 2 * s, iy, 1 * s, 2 * s);
      ctx.fillRect(ix + 4 * s, iy, 1 * s, 2 * s);
      break;
    }
    case 'water': {
      // Droplet
      ctx.fillStyle = '#4488FF';
      ctx.fillRect(ix + 2 * s, iy + 3 * s, 3 * s, 3 * s);
      ctx.fillRect(ix + 3 * s, iy + 1 * s, 1 * s, 2 * s);
      break;
    }
    case 'stretch': {
      // Up arrows
      ctx.fillStyle = '#88FFAA';
      ctx.fillRect(ix + 2 * s, iy + 1 * s, 1 * s, 4 * s);
      ctx.fillRect(ix + 1 * s, iy + 2 * s, 1 * s, 1 * s);
      ctx.fillRect(ix + 3 * s, iy + 2 * s, 1 * s, 1 * s);
      ctx.fillRect(ix + 5 * s, iy + 1 * s, 1 * s, 4 * s);
      ctx.fillRect(ix + 4 * s, iy + 2 * s, 1 * s, 1 * s);
      ctx.fillRect(ix + 6 * s, iy + 2 * s, 1 * s, 1 * s);
      break;
    }
    case 'whiteboard': {
      // Small pencil
      ctx.fillStyle = '#FFEE88';
      ctx.fillRect(ix + 1 * s, iy + 1 * s, 2 * s, 5 * s);
      ctx.fillStyle = '#FF8844';
      ctx.fillRect(ix + 1 * s, iy + 6 * s, 2 * s, 1 * s);
      ctx.fillStyle = '#EEEEEE';
      ctx.fillRect(ix + 1 * s, iy, 2 * s, 1 * s);
      break;
    }
    case 'meeting': {
      // Tiny table
      ctx.fillStyle = '#8A6040';
      ctx.fillRect(ix + 1 * s, iy + 3 * s, 5 * s, 1 * s);
      ctx.fillRect(ix + 1 * s, iy + 4 * s, 1 * s, 2 * s);
      ctx.fillRect(ix + 5 * s, iy + 4 * s, 1 * s, 2 * s);
      // Seats
      ctx.fillStyle = '#555';
      ctx.fillRect(ix + 1 * s, iy + 1 * s, 2 * s, 1 * s);
      ctx.fillRect(ix + 4 * s, iy + 1 * s, 2 * s, 1 * s);
      break;
    }
  }

  ctx.restore();}

function drawZzz(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  ctx.save();
  ctx.fillStyle = '#9090CC';
  ctx.font = `${3 * s}px monospace`;
  ctx.globalAlpha = 0.8;
  ctx.fillText('z', x, y);
  ctx.globalAlpha = 0.5;
  ctx.fillText('z', x + 4 * s, y - 4 * s);
  ctx.globalAlpha = 0.3;
  ctx.fillText('Z', x + 8 * s, y - 8 * s);
  ctx.globalAlpha = 1;
  ctx.restore();
}

// ─── DESKS ────────────────────────────────────────────────────────────────────

export function drawDesk(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  color: string,
  label: string,
  agentId?: string,
  _appearance?: AgentAppearance,
): void {
  void _appearance;
  const dx = x * s;
  const dy = (y + 14) * s;

  // Chair (behind desk, below character)
  drawChair(ctx, dx + 4 * s, dy + 18 * s, color);

  // Desk surface
  ctx.fillStyle = '#7A6245';
  ctx.fillRect(dx - 4 * s, dy + 8 * s, CHAR_W * s + 8 * s, 3 * s);
  // Surface highlight
  ctx.fillStyle = '#9A8265';
  ctx.fillRect(dx - 4 * s, dy + 8 * s, CHAR_W * s + 8 * s, 1 * s);
  // Surface shadow
  ctx.fillStyle = '#5A4225';
  ctx.fillRect(dx - 4 * s, dy + 11 * s, CHAR_W * s + 8 * s, 1 * s);

  // Desk body
  ctx.fillStyle = '#6A5235';
  ctx.fillRect(dx - 4 * s, dy + 12 * s, CHAR_W * s + 8 * s, 6 * s);

  // Desk legs
  ctx.fillStyle = '#4A3215';
  ctx.fillRect(dx - 3 * s, dy + 12 * s, 2 * s, 8 * s);
  ctx.fillRect(dx + CHAR_W * s + 3 * s, dy + 12 * s, 2 * s, 8 * s);

  // Draw agent-specific desk props
  switch (agentId) {
    case 'main': drawJarvisDesk(ctx, dx, dy); break;
    case 'analista': drawAnalistaDesk(ctx, dx, dy); break;
    case 'rafael': drawFelipeDesk(ctx, dx, dy); break;
    case 'bibliotecario': drawBibliotecarioDesk(ctx, dx, dy); break;
    default: drawGenericMonitor(ctx, dx, dy); break;
  }

  // Name plate
  ctx.fillStyle = color;
  ctx.fillRect(dx - 4 * s, dy + 20 * s, CHAR_W * s + 8 * s, 5 * s);
  ctx.fillStyle = darken(color, 40);
  ctx.fillRect(dx - 4 * s, dy + 20 * s, CHAR_W * s + 8 * s, 1 * s);
  ctx.fillStyle = '#FFFFFF';
  ctx.font = `bold ${3 * s}px monospace`;
  ctx.textAlign = 'center';
  ctx.fillText(label, dx + (CHAR_W / 2) * s, dy + 24 * s);
  ctx.textAlign = 'start';
}

function drawChair(ctx: CanvasRenderingContext2D, x: number, y: number, accentColor: string): void {
  // Chair seat
  ctx.fillStyle = '#1A1A2E';
  ctx.fillRect(x, y, 9 * s, 2 * s);
  // Seat cushion
  ctx.fillStyle = accentColor;
  ctx.globalAlpha = 0.4;
  ctx.fillRect(x + 1 * s, y, 7 * s, 2 * s);
  ctx.globalAlpha = 1;
  // Chair base/stem
  ctx.fillStyle = '#333';
  ctx.fillRect(x + 4 * s, y + 2 * s, 2 * s, 3 * s);
  // Wheels
  ctx.fillStyle = '#444';
  ctx.fillRect(x + 1 * s, y + 5 * s, 2 * s, 1 * s);
  ctx.fillRect(x + 6 * s, y + 5 * s, 2 * s, 1 * s);
}

function drawJarvisDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Left monitor (smaller)
  ctx.fillStyle = '#111';
  ctx.fillRect(dx - 3 * s, dy + 2 * s, 8 * s, 6 * s);
  ctx.fillStyle = '#001830';
  ctx.fillRect(dx - 2 * s, dy + 3 * s, 6 * s, 4 * s);
  // Screen content — dashboard lines
  ctx.fillStyle = '#0066FF';
  ctx.globalAlpha = 0.8;
  ctx.fillRect(dx - 1 * s, dy + 4 * s, 4 * s, 1 * s);
  ctx.fillRect(dx - 1 * s, dy + 6 * s, 2 * s, 1 * s);
  ctx.globalAlpha = 1;

  // Right monitor (main)
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 6 * s, dy + 1 * s, 10 * s, 7 * s);
  ctx.fillStyle = '#001020';
  ctx.fillRect(dx + 7 * s, dy + 2 * s, 8 * s, 5 * s);
  // Green terminal text
  ctx.fillStyle = '#00FF66';
  ctx.globalAlpha = 0.7;
  for (let i = 0; i < 3; i++) {
    ctx.fillRect(dx + 8 * s, dy + (3 + i) * s, (3 + i) * s, 1 * s);
  }
  ctx.globalAlpha = 1;

  // Monitor stands
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 1 * s, dy + 8 * s, 2 * s, 1 * s);
  ctx.fillRect(dx + 10 * s, dy + 8 * s, 2 * s, 1 * s);

  // Keyboard
  ctx.fillStyle = '#1A1A2A';
  ctx.fillRect(dx + 2 * s, dy + 9 * s, 10 * s, 2 * s);

  // Coffee mug
  ctx.fillStyle = '#2A4A2A';
  ctx.fillRect(dx + 14 * s, dy + 7 * s, 3 * s, 3 * s);
  ctx.fillStyle = '#6AAA60';
  ctx.fillRect(dx + 14 * s, dy + 7 * s, 3 * s, 1 * s);
}

function drawAnalistaDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Monitor with news feed
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 1 * s, dy + 1 * s, 12 * s, 7 * s);
  ctx.fillStyle = '#1A1A2A';
  ctx.fillRect(dx + 2 * s, dy + 2 * s, 10 * s, 5 * s);
  // RSS/news lines
  ctx.fillStyle = '#FFAA00';
  ctx.globalAlpha = 0.9;
  ctx.fillRect(dx + 3 * s, dy + 3 * s, 8 * s, 1 * s);
  ctx.globalAlpha = 0.6;
  ctx.fillRect(dx + 3 * s, dy + 5 * s, 5 * s, 1 * s);
  ctx.fillRect(dx + 3 * s, dy + 6 * s, 6 * s, 1 * s);
  ctx.globalAlpha = 1;
  // Monitor stand
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 6 * s, dy + 8 * s, 2 * s, 1 * s);

  // Stack of newspapers (left side)
  ctx.fillStyle = '#D4C4A0';
  ctx.fillRect(dx - 4 * s, dy + 3 * s, 5 * s, 6 * s);
  ctx.fillStyle = '#C4B490';
  ctx.fillRect(dx - 4 * s, dy + 4 * s, 5 * s, 1 * s);
  ctx.fillRect(dx - 4 * s, dy + 6 * s, 5 * s, 1 * s);
  // Paper lines
  ctx.fillStyle = '#888';
  ctx.globalAlpha = 0.5;
  ctx.fillRect(dx - 3 * s, dy + 5 * s, 3 * s, 1 * s);
  ctx.globalAlpha = 1;

  // Coffee mug (right)
  ctx.fillStyle = '#3A2A1A';
  ctx.fillRect(dx + 14 * s, dy + 7 * s, 3 * s, 3 * s);
  ctx.fillStyle = '#5A3A20';
  ctx.fillRect(dx + 14 * s, dy + 7 * s, 3 * s, 1 * s);
  ctx.fillStyle = '#8A5A30';
  ctx.fillRect(dx + 17 * s, dy + 8 * s, 1 * s, 1 * s);

  // Keyboard
  ctx.fillStyle = '#222';
  ctx.fillRect(dx + 2 * s, dy + 9 * s, 10 * s, 2 * s);
}

function drawFelipeDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Monitor with Telegram-style interface
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 2 * s, dy + 1 * s, 11 * s, 7 * s);
  ctx.fillStyle = '#0A1A2A';
  ctx.fillRect(dx + 3 * s, dy + 2 * s, 9 * s, 5 * s);
  // Blue message bubbles
  ctx.fillStyle = '#1C7ACC';
  ctx.globalAlpha = 0.9;
  ctx.fillRect(dx + 8 * s, dy + 3 * s, 3 * s, 1 * s);
  ctx.fillRect(dx + 7 * s, dy + 5 * s, 4 * s, 1 * s);
  ctx.globalAlpha = 0.5;
  ctx.fillStyle = '#444';
  ctx.fillRect(dx + 4 * s, dy + 4 * s, 3 * s, 1 * s);
  ctx.fillRect(dx + 4 * s, dy + 6 * s, 3 * s, 1 * s);
  ctx.globalAlpha = 1;
  // Monitor stand
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 7 * s, dy + 8 * s, 2 * s, 1 * s);

  // Desk phone (left)
  ctx.fillStyle = '#1A1A1A';
  ctx.fillRect(dx - 4 * s, dy + 4 * s, 5 * s, 6 * s);
  ctx.fillStyle = '#333';
  ctx.fillRect(dx - 3 * s, dy + 5 * s, 3 * s, 4 * s);
  // Phone buttons
  ctx.fillStyle = '#555';
  ctx.fillRect(dx - 3 * s, dy + 6 * s, 1 * s, 1 * s);
  ctx.fillRect(dx - 1 * s, dy + 6 * s, 1 * s, 1 * s);
  ctx.fillStyle = '#00AA44';
  ctx.fillRect(dx - 2 * s, dy + 5 * s, 1 * s, 1 * s);

  // Tablet/phone device
  ctx.fillStyle = '#222';
  ctx.fillRect(dx + 14 * s, dy + 4 * s, 3 * s, 5 * s);
  ctx.fillStyle = '#1C4A7A';
  ctx.fillRect(dx + 14 * s, dy + 5 * s, 3 * s, 3 * s);

  // Keyboard
  ctx.fillStyle = '#1A1A2A';
  ctx.fillRect(dx + 2 * s, dy + 9 * s, 10 * s, 2 * s);
}

function drawBibliotecarioDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Monitor with graph/network view
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 2 * s, dy + 1 * s, 11 * s, 7 * s);
  ctx.fillStyle = '#1A0A2A';
  ctx.fillRect(dx + 3 * s, dy + 2 * s, 9 * s, 5 * s);
  // Network graph nodes
  ctx.fillStyle = '#9955FF';
  ctx.globalAlpha = 0.8;
  ctx.fillRect(dx + 7 * s, dy + 3 * s, 2 * s, 2 * s);
  ctx.globalAlpha = 0.6;
  ctx.fillRect(dx + 4 * s, dy + 4 * s, 2 * s, 2 * s);
  ctx.fillRect(dx + 10 * s, dy + 5 * s, 2 * s, 2 * s);
  // Edges
  ctx.strokeStyle = '#9955FF';
  ctx.globalAlpha = 0.4;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo((dx + 9) * 1, (dy + 4) * 1);  // simplified line
  ctx.stroke();
  ctx.globalAlpha = 1;
  // Monitor stand
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 7 * s, dy + 8 * s, 2 * s, 1 * s);

  // Stack of books (left)
  const bookColors = ['#8B0000', '#00558B', '#006B00', '#8B6B00'];
  for (let i = 0; i < 4; i++) {
    ctx.fillStyle = bookColors[i];
    ctx.fillRect(dx - 4 * s, dy + (2 + i * 2) * s, 4 * s, 2 * s);
    ctx.fillStyle = lighten(bookColors[i], 20);
    ctx.fillRect(dx - 4 * s, dy + (2 + i * 2) * s, 4 * s, 1 * s);
  }

  // Magnifying glass
  ctx.fillStyle = '#666';
  ctx.fillRect(dx + 14 * s, dy + 4 * s, 3 * s, 3 * s);
  ctx.fillStyle = '#AACCFF';
  ctx.fillRect(dx + 15 * s, dy + 5 * s, 1 * s, 1 * s);

  // Keyboard
  ctx.fillStyle = '#222';
  ctx.fillRect(dx + 2 * s, dy + 9 * s, 10 * s, 2 * s);
}

function drawGenericMonitor(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 2 * s, dy + 2 * s, 12 * s, 6 * s);
  ctx.fillStyle = '#1a3a5a';
  ctx.fillRect(dx + 3 * s, dy + 3 * s, 10 * s, 4 * s);
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 7 * s, dy + 8 * s, 2 * s, 1 * s);
}

// ─── FURNITURE (for background scene) ─────────────────────────────────────────

export function drawServerRack(ctx: CanvasRenderingContext2D, x: number, y: number, frame: number): void {
  const bx = x * s, by = y * s;
  const w = 14 * s, h = 80 * s;

  // Cabinet body
  ctx.fillStyle = '#1A1A22';
  ctx.fillRect(bx, by, w, h);
  ctx.fillStyle = '#111118';
  ctx.fillRect(bx, by, 1 * s, h);
  ctx.fillStyle = '#2A2A35';
  ctx.fillRect(bx + w - 1 * s, by, 1 * s, h);

  // Rack units
  for (let i = 0; i < 10; i++) {
    const ry = by + i * 8 * s;
    ctx.fillStyle = i % 2 === 0 ? '#1E1E28' : '#181820';
    ctx.fillRect(bx + 1 * s, ry, w - 2 * s, 7 * s);

    // LED indicators
    const ledColor = i === 2 || i === 5 ? '#00FF66' : i === 7 ? (frame % 4 < 2 ? '#FFAA00' : '#885500') : '#00AA44';
    ctx.fillStyle = ledColor;
    ctx.fillRect(bx + 2 * s, ry + 3 * s, 2 * s, 1 * s);

    // Vent slots
    ctx.fillStyle = '#0A0A12';
    for (let v = 0; v < 4; v++) {
      ctx.fillRect(bx + 5 * s + v * 2 * s, ry + 2 * s, 1 * s, 3 * s);
    }
  }

  // Cabinet handles
  ctx.fillStyle = '#444';
  ctx.fillRect(bx + w - 3 * s, by + 3 * s, 1 * s, h - 6 * s);
}

export function drawWhiteboard(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  const w = 55 * s, h = 28 * s;

  // Frame
  ctx.fillStyle = '#555560';
  ctx.fillRect(bx, by, w, h);
  // Board
  ctx.fillStyle = '#F0EEE8';
  ctx.fillRect(bx + 2 * s, by + 2 * s, w - 4 * s, h - 4 * s);

  // Drawn content — schedule/diagram
  ctx.fillStyle = '#3344AA';
  ctx.font = `bold ${3 * s}px monospace`;
  ctx.fillText('SYSTEM', bx + 4 * s, by + 8 * s);
  ctx.fillStyle = '#888';
  ctx.fillRect(bx + 4 * s, by + 10 * s, 30 * s, 1 * s);

  // Flow diagram boxes
  ctx.strokeStyle = '#3344AA';
  ctx.lineWidth = 1;
  ctx.strokeRect(bx + 4 * s, by + 13 * s, 10 * s, 6 * s);
  ctx.strokeRect(bx + 20 * s, by + 13 * s, 10 * s, 6 * s);
  ctx.strokeRect(bx + 36 * s, by + 13 * s, 10 * s, 6 * s);
  // Arrows
  ctx.fillStyle = '#3344AA';
  ctx.fillRect(bx + 14 * s, by + 16 * s, 6 * s, 1 * s);
  ctx.fillRect(bx + 30 * s, by + 16 * s, 6 * s, 1 * s);
  // Labels in boxes
  ctx.font = `${2 * s}px monospace`;
  ctx.fillStyle = '#333';
  ctx.fillText('JRV', bx + 5 * s, by + 17 * s);
  ctx.fillText('ANL', bx + 21 * s, by + 17 * s);
  ctx.fillText('BIB', bx + 37 * s, by + 17 * s);

  // Marker tray
  ctx.fillStyle = '#888';
  ctx.fillRect(bx + 2 * s, by + h - 4 * s, w - 4 * s, 3 * s);
  // Markers
  const markerColors = ['#FF4444', '#4444FF', '#444'];
  for (let i = 0; i < 3; i++) {
    ctx.fillStyle = markerColors[i];
    ctx.fillRect(bx + 4 * s + i * 5 * s, by + h - 3 * s, 4 * s, 2 * s);
  }
}

export function drawCorkboard(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  const w = 65 * s, h = 30 * s;

  // Frame
  ctx.fillStyle = '#6B4A20';
  ctx.fillRect(bx, by, w, h);
  // Cork surface
  ctx.fillStyle = '#C8A06A';
  ctx.fillRect(bx + 2 * s, by + 2 * s, w - 4 * s, h - 4 * s);
  // Cork texture
  ctx.fillStyle = '#B89060';
  for (let tx = 0; tx < 7; tx++) {
    for (let ty = 0; ty < 4; ty++) {
      if ((tx + ty) % 3 === 0) {
        ctx.fillRect(bx + 2 * s + tx * 9 * s, by + 2 * s + ty * 7 * s, 4 * s, 3 * s);
      }
    }
  }

  // Pinned papers
  const papers = [
    { px: 4, py: 4, w: 16, h: 12, color: '#FFFFD0', rot: -2 },
    { px: 24, py: 5, w: 14, h: 10, color: '#FFE0E0', rot: 1 },
    { px: 42, py: 3, w: 18, h: 14, color: '#E0FFE0', rot: -1 },
    { px: 8, py: 17, w: 12, h: 9, color: '#E0E0FF', rot: 2 },
    { px: 30, py: 16, w: 16, h: 11, color: '#FFE8C0', rot: -1 },
  ];
  for (const p of papers) {
    ctx.fillStyle = p.color;
    ctx.fillRect(bx + (2 + p.px) * s, by + (2 + p.py) * s, p.w * s, p.h * s);
    // Lines on paper
    ctx.fillStyle = '#AAA';
    ctx.globalAlpha = 0.5;
    for (let l = 0; l < 3; l++) {
      ctx.fillRect(bx + (4 + p.px) * s, by + (5 + p.py + l * 3) * s, (p.w - 4) * s, 1 * s);
    }
    ctx.globalAlpha = 1;
    // Pin
    ctx.fillStyle = '#FF4444';
    ctx.fillRect(bx + (2 + p.px + p.w / 2) * s, by + (2 + p.py) * s, 2 * s, 2 * s);
  }
}

export function drawBookshelf(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  const w = 22 * s, h = 80 * s;

  // Shelf frame
  ctx.fillStyle = '#5A3A1A';
  ctx.fillRect(bx, by, w, h);
  ctx.fillStyle = '#4A2A0A';
  ctx.fillRect(bx, by, 2 * s, h);
  ctx.fillRect(bx + w - 2 * s, by, 2 * s, h);

  // Shelves (4)
  const allBooks = [
    ['#8B0000', '#00558B', '#006B00', '#8B6B00', '#5A008B'],
    ['#AA3300', '#0033AA', '#AA8800', '#007755', '#880044'],
    ['#CC4400', '#004488', '#448800', '#AA6600', '#220099'],
    ['#BB2200', '#002299', '#006622', '#CC8800', '#660088'],
  ];
  for (let shelf = 0; shelf < 4; shelf++) {
    const sy = by + shelf * 20 * s;
    // Shelf board
    ctx.fillStyle = '#6A4A2A';
    ctx.fillRect(bx + 2 * s, sy + 17 * s, w - 4 * s, 2 * s);
    ctx.fillStyle = '#7A5A3A';
    ctx.fillRect(bx + 2 * s, sy + 17 * s, w - 4 * s, 1 * s);

    // Books
    const books = allBooks[shelf];
    let bkx = bx + 3 * s;
    for (let b = 0; b < books.length; b++) {
      const bw = (2 + (b % 2)) * s;
      const bh = (10 + (b % 3) * 2) * s;
      ctx.fillStyle = books[b];
      ctx.fillRect(bkx, sy + 17 * s - bh, bw, bh);
      ctx.fillStyle = lighten(books[b], 25);
      ctx.fillRect(bkx, sy + 17 * s - bh, bw, 1 * s);
      ctx.fillStyle = darken(books[b], 20);
      ctx.fillRect(bkx, sy + 17 * s - bh, 1 * s, bh);
      bkx += bw + 1 * s;
    }
  }
}

export function drawFilingCabinet(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  ctx.fillStyle = '#2A2A35';
  ctx.fillRect(bx, by, 14 * s, 40 * s);
  ctx.fillStyle = '#1A1A25';
  ctx.fillRect(bx, by, 1 * s, 40 * s);

  for (let d = 0; d < 3; d++) {
    const dy2 = by + d * 13 * s + 2 * s;
    ctx.fillStyle = '#333340';
    ctx.fillRect(bx + 1 * s, dy2, 12 * s, 11 * s);
    // Drawer handle
    ctx.fillStyle = '#888898';
    ctx.fillRect(bx + 4 * s, dy2 + 4 * s, 6 * s, 2 * s);
    ctx.fillStyle = '#AAAACC';
    ctx.fillRect(bx + 4 * s, dy2 + 4 * s, 6 * s, 1 * s);
    // Label
    ctx.fillStyle = '#FFFFFF';
    ctx.fillRect(bx + 3 * s, dy2 + 7 * s, 8 * s, 3 * s);
    ctx.fillStyle = '#666';
    ctx.font = `${2 * s}px monospace`;
    ctx.fillText('A-Z', bx + 5 * s, dy2 + 9 * s);
  }
}

export function drawCoffeeStation(ctx: CanvasRenderingContext2D, x: number, y: number, frame: number): void {
  const bx = x * s, by = y * s;

  // Counter
  ctx.fillStyle = '#4A3520';
  ctx.fillRect(bx, by + 10 * s, 22 * s, 5 * s);
  ctx.fillStyle = '#6A5540';
  ctx.fillRect(bx, by + 10 * s, 22 * s, 1 * s);

  // Coffee machine body
  ctx.fillStyle = '#1A1A22';
  ctx.fillRect(bx + 1 * s, by + 2 * s, 14 * s, 10 * s);
  ctx.fillStyle = '#222230';
  ctx.fillRect(bx + 2 * s, by + 3 * s, 12 * s, 8 * s);

  // Display
  ctx.fillStyle = '#002200';
  ctx.fillRect(bx + 3 * s, by + 4 * s, 7 * s, 4 * s);
  ctx.fillStyle = '#00AA00';
  ctx.globalAlpha = 0.8;
  ctx.font = `${2 * s}px monospace`;
  ctx.fillText('READY', bx + 3 * s, by + 7 * s);
  ctx.globalAlpha = 1;

  // Buttons
  ctx.fillStyle = '#333';
  ctx.fillRect(bx + 11 * s, by + 5 * s, 2 * s, 2 * s);
  ctx.fillStyle = '#AA0000';
  ctx.fillRect(bx + 11 * s, by + 5 * s, 2 * s, 2 * s);

  // Steam
  if (frame % 4 < 2) {
    ctx.fillStyle = 'rgba(200,200,200,0.3)';
    ctx.fillRect(bx + 6 * s, by, 2 * s, 2 * s);
    ctx.fillRect(bx + 8 * s, by - 1 * s, 2 * s, 2 * s);
  }

  // Cup tray
  ctx.fillStyle = '#333';
  ctx.fillRect(bx + 2 * s, by + 10 * s, 12 * s, 1 * s);

  // Cup
  ctx.fillStyle = '#F5F5F5';
  ctx.fillRect(bx + 5 * s, by + 7 * s, 4 * s, 4 * s);
  ctx.fillStyle = '#3A1A00';
  ctx.fillRect(bx + 5 * s, by + 7 * s, 4 * s, 2 * s);

  // Mugs on counter
  const mugColors = ['#CC4400', '#004488', '#448800'];
  for (let m = 0; m < 3; m++) {
    ctx.fillStyle = mugColors[m];
    ctx.fillRect(bx + (15 + m * 2) * s, by + 7 * s, 2 * s, 3 * s);
    ctx.fillStyle = lighten(mugColors[m], 20);
    ctx.fillRect(bx + (15 + m * 2) * s, by + 7 * s, 2 * s, 1 * s);
  }
}

export function drawPlant(ctx: CanvasRenderingContext2D, x: number, y: number, large = false): void {
  const bx = x * s, by = y * s;
  const size = large ? 2 : 1;

  // Pot
  ctx.fillStyle = '#8B4513';
  ctx.fillRect(bx + 2 * s, by + 8 * size * s, 6 * size * s, 6 * size * s);
  ctx.fillStyle = '#6B3510';
  ctx.fillRect(bx + 2 * s, by + 8 * size * s, 6 * size * s, 1 * s);
  // Soil
  ctx.fillStyle = '#3A2010';
  ctx.fillRect(bx + 3 * s, by + 8 * size * s, 4 * size * s, 2 * s);

  // Leaves
  const leafColor = '#1A6A1A';
  const leafLight = '#2A8A2A';
  if (large) {
    // Big leafy plant
    ctx.fillStyle = leafColor;
    ctx.fillRect(bx + 4 * s, by + 2 * s, 2 * s, 6 * s);
    ctx.fillRect(bx, by + 2 * s, 10 * s, 4 * s);
    ctx.fillRect(bx + 1 * s, by, 8 * s, 4 * s);
    ctx.fillRect(bx - 2 * s, by + 4 * s, 6 * s, 3 * s);
    ctx.fillRect(bx + 6 * s, by + 3 * s, 6 * s, 3 * s);
    ctx.fillStyle = leafLight;
    ctx.fillRect(bx + 4 * s, by + 2 * s, 1 * s, 4 * s);
    ctx.fillRect(bx + 1 * s, by + 2 * s, 7 * s, 1 * s);
  } else {
    // Small cactus/succulent
    ctx.fillStyle = '#2A7A2A';
    ctx.fillRect(bx + 3 * s, by + 3 * s, 4 * s, 5 * s);
    ctx.fillRect(bx + 1 * s, by + 5 * s, 3 * s, 3 * s);
    ctx.fillRect(bx + 6 * s, by + 4 * s, 3 * s, 3 * s);
    ctx.fillStyle = '#3A9A3A';
    ctx.fillRect(bx + 4 * s, by + 3 * s, 1 * s, 5 * s);
    // Spines
    ctx.fillStyle = '#EEE';
    ctx.fillRect(bx + 2 * s, by + 5 * s, 1 * s, 1 * s);
    ctx.fillRect(bx + 7 * s, by + 4 * s, 1 * s, 1 * s);
  }
}

export function drawMeetingTable(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;

  // Table surface (rounded rect approximation)
  ctx.fillStyle = '#5A4030';
  ctx.fillRect(bx + 4 * s, by, 32 * s, 14 * s);
  ctx.fillRect(bx, by + 3 * s, 40 * s, 8 * s);
  ctx.fillRect(bx + 2 * s, by + 1 * s, 36 * s, 12 * s);
  // Surface highlight
  ctx.fillStyle = '#7A6050';
  ctx.fillRect(bx + 4 * s, by, 32 * s, 1 * s);
  ctx.fillRect(bx + 2 * s, by + 1 * s, 36 * s, 1 * s);
  // Table legs
  ctx.fillStyle = '#3A2010';
  ctx.fillRect(bx + 4 * s, by + 12 * s, 3 * s, 4 * s);
  ctx.fillRect(bx + 33 * s, by + 12 * s, 3 * s, 4 * s);

  // Chairs around table
  ctx.fillStyle = '#1A1A2E';
  // Top chairs
  ctx.fillRect(bx + 5 * s, by - 5 * s, 8 * s, 4 * s);
  ctx.fillRect(bx + 27 * s, by - 5 * s, 8 * s, 4 * s);
  // Bottom chairs
  ctx.fillRect(bx + 5 * s, by + 14 * s, 8 * s, 4 * s);
  ctx.fillRect(bx + 27 * s, by + 14 * s, 8 * s, 4 * s);

  // Items on table
  // Laptop
  ctx.fillStyle = '#222';
  ctx.fillRect(bx + 8 * s, by + 3 * s, 8 * s, 5 * s);
  ctx.fillStyle = '#001830';
  ctx.fillRect(bx + 9 * s, by + 4 * s, 6 * s, 3 * s);
  // Water glass
  ctx.fillStyle = 'rgba(200,230,255,0.5)';
  ctx.fillRect(bx + 20 * s, by + 4 * s, 3 * s, 5 * s);
  ctx.strokeStyle = 'rgba(180,210,240,0.8)';
  ctx.lineWidth = 1;
  ctx.strokeRect(bx + 20 * s, by + 4 * s, 3 * s, 5 * s);
  // Papers
  ctx.fillStyle = '#F0EEE8';
  ctx.fillRect(bx + 25 * s, by + 3 * s, 7 * s, 5 * s);
  ctx.fillStyle = '#CCC';
  ctx.fillRect(bx + 26 * s, by + 5 * s, 5 * s, 1 * s);
  ctx.fillRect(bx + 26 * s, by + 7 * s, 3 * s, 1 * s);
}

export function drawWaterCooler(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  // Body
  ctx.fillStyle = '#2A2A35';
  ctx.fillRect(bx + 2 * s, by + 5 * s, 10 * s, 18 * s);
  // Water bottle (blue tinted)
  ctx.fillStyle = 'rgba(100,160,220,0.6)';
  ctx.fillRect(bx + 3 * s, by, 8 * s, 8 * s);
  ctx.strokeStyle = 'rgba(100,160,220,0.9)';
  ctx.lineWidth = 1;
  ctx.strokeRect(bx + 3 * s, by, 8 * s, 8 * s);
  // Taps
  ctx.fillStyle = '#0066CC';
  ctx.fillRect(bx + 3 * s, by + 15 * s, 3 * s, 2 * s);
  ctx.fillStyle = '#CC0000';
  ctx.fillRect(bx + 8 * s, by + 15 * s, 3 * s, 2 * s);
  // Base
  ctx.fillStyle = '#1A1A22';
  ctx.fillRect(bx + 1 * s, by + 22 * s, 12 * s, 3 * s);
}

export function drawCeilingLight(ctx: CanvasRenderingContext2D, x: number, y: number, on: boolean, frame: number): void {
  const bx = x * s, by = y * s;
  // Fixture
  ctx.fillStyle = '#333';
  ctx.fillRect(bx - 5 * s, by, 10 * s, 2 * s);
  // Bulb
  if (on) {
    ctx.fillStyle = '#FFFFC0';
    ctx.fillRect(bx - 3 * s, by + 2 * s, 6 * s, 3 * s);
    // Glow
    const pulse = 0.15 + 0.05 * Math.sin(frame * 0.1);
    const grd = ctx.createRadialGradient(bx, by + 3 * s, 0, bx, by + 3 * s, 20 * s);
    grd.addColorStop(0, `rgba(255,255,180,${pulse})`);
    grd.addColorStop(1, 'rgba(255,255,180,0)');
    ctx.fillStyle = grd;
    ctx.fillRect(bx - 20 * s, by, 40 * s, 40 * s);
  } else {
    ctx.fillStyle = '#555';
    ctx.fillRect(bx - 3 * s, by + 2 * s, 6 * s, 3 * s);
  }
}

export function drawClock(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  const now = new Date();
  // Clock face
  ctx.fillStyle = '#EEE';
  ctx.fillRect(bx, by, 12 * s, 12 * s);
  ctx.strokeStyle = '#333';
  ctx.lineWidth = 1;
  ctx.strokeRect(bx, by, 12 * s, 12 * s);
  // Center
  ctx.fillStyle = '#333';
  ctx.fillRect(bx + 5 * s, by + 5 * s, 2 * s, 2 * s);
  // Hour hand (simplified)
  const h = now.getHours() % 12;
  ctx.fillStyle = '#333';
  if (h < 3) ctx.fillRect(bx + 5 * s, by + 2 * s, 2 * s, 4 * s);
  else if (h < 6) ctx.fillRect(bx + 7 * s, by + 5 * s, 3 * s, 2 * s);
  else if (h < 9) ctx.fillRect(bx + 5 * s, by + 7 * s, 2 * s, 4 * s);
  else ctx.fillRect(bx + 1 * s, by + 5 * s, 4 * s, 2 * s);
  // Frame
  ctx.fillStyle = '#5A3A1A';
  ctx.fillRect(bx - 1 * s, by - 1 * s, 14 * s, 1 * s);
  ctx.fillRect(bx - 1 * s, by - 1 * s, 1 * s, 14 * s);
  ctx.fillRect(bx - 1 * s, by + 12 * s, 14 * s, 1 * s);
  ctx.fillRect(bx + 12 * s, by - 1 * s, 1 * s, 14 * s);
}

// ─── HARDWARE PANEL ───────────────────────────────────────────────────────────

export function drawHardwarePanel(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  hw: HardwareState | null,
  frame: number,
): void {
  const bx = x * s, by = y * s;
  const pw = 50 * s, ph = 80 * s;

  // Panel background
  ctx.fillStyle = '#111120';
  ctx.fillRect(bx, by, pw, ph);

  // Panel border
  ctx.fillStyle = '#2A2A44';
  ctx.fillRect(bx, by, pw, 1 * s);
  ctx.fillRect(bx, by, 1 * s, ph);
  ctx.fillRect(bx + pw - 1 * s, by, 1 * s, ph);
  ctx.fillRect(bx, by + ph - 1 * s, pw, 1 * s);

  // Header bar
  ctx.fillStyle = '#1A1A30';
  ctx.fillRect(bx + 1 * s, by + 1 * s, pw - 2 * s, 9 * s);

  // Header indicator dot
  ctx.fillStyle = hw ? '#00CC44' : '#444466';
  ctx.fillRect(bx + 3 * s, by + 3 * s, 3 * s, 3 * s);

  // Header divider
  ctx.fillStyle = '#2A2A44';
  ctx.fillRect(bx + 1 * s, by + 10 * s, pw - 2 * s, 1 * s);

  // "SYS MONITOR" label (pixel text using fillRect blocks)
  ctx.fillStyle = '#9999CC';
  ctx.font = `bold ${2.5 * s}px monospace`;
  ctx.fillText('SYS MONITOR', bx + 8 * s, by + 8 * s);

  if (!hw) {
    // Offline indicator
    ctx.fillStyle = '#444466';
    ctx.font = `${2.5 * s}px monospace`;
    ctx.fillText('OFFLINE', bx + 10 * s, by + 40 * s);
    return;
  }

  // Helper: draw a labeled fill bar
  // row: 0-based index of the bar
  function drawBar(
    label: string,
    pct: number,
    color: string,
    rowIdx: number,
    extraLabel?: string,
  ): void {
    const rowY = by + (13 + rowIdx * 14) * s;
    const barX = bx + 2 * s;
    const barW = pw - 4 * s;
    const labelW = 10 * s;
    const barInnerX = barX + labelW;
    const barInnerW = barW - labelW - 8 * s;

    // Row label
    ctx.fillStyle = '#7777AA';
    ctx.font = `${2 * s}px monospace`;
    ctx.fillText(label, barX, rowY + 5 * s);

    // Bar background
    ctx.fillStyle = '#1E1E38';
    ctx.fillRect(barInnerX, rowY, barInnerW, 6 * s);

    // Bar fill
    const fillW = Math.round((pct / 100) * barInnerW);
    ctx.fillStyle = color;
    ctx.fillRect(barInnerX, rowY, fillW, 6 * s);

    // Pct text
    ctx.fillStyle = '#AAAACC';
    ctx.font = `${2 * s}px monospace`;
    const pctStr = extraLabel ?? `${pct}%`;
    ctx.fillText(pctStr, barInnerX + barInnerW + 2 * s, rowY + 5 * s);
  }

  // CPU bar
  const cpuColor = '#00CC44';
  drawBar('CPU', hw.cpu.pct, cpuColor, 0);

  // RAM bar
  const ramPct = hw.mem.pct;
  const ramColor = ramPct > 85 ? '#FF4444' : '#FFAA00';
  drawBar('RAM', ramPct, ramColor, 1);

  // GPU util bar (row 2)
  if (hw.gpu) {
    const gpuUtilColor = hw.gpu.utilPct > 80 ? '#FF4444' : '#4488FF';
    // GPU glow blink when util > 50%
    if (hw.gpu.utilPct > 50 && frame % 4 < 2) {
      ctx.globalAlpha = 0.15;
      ctx.fillStyle = gpuUtilColor;
      ctx.fillRect(bx + 1 * s, by + (13 + 2 * 14) * s - 1 * s, pw - 2 * s, 8 * s);
      ctx.globalAlpha = 1;
    }
    drawBar('GPU', hw.gpu.utilPct, gpuUtilColor, 2, `${hw.gpu.utilPct}%`);

    // VRAM bar (row 3)
    const vramPct = Math.round((hw.gpu.memUsedMB / hw.gpu.memTotalMB) * 100);
    const vramColor = vramPct > 90 ? '#FF4444' : '#9955FF';
    drawBar('VRAM', vramPct, vramColor, 3);

    // GPU temp warning
    if (hw.gpu.tempC > 70) {
      ctx.fillStyle = frame % 2 === 0 ? '#FF4444' : '#FF8800';
      ctx.font = `${2 * s}px monospace`;
      ctx.fillText(`GPU HOT ${hw.gpu.tempC}C`, bx + 3 * s, by + 70 * s);
    } else {
      ctx.fillStyle = '#555577';
      ctx.font = `${2 * s}px monospace`;
      ctx.fillText(`${hw.gpu.tempC}C`, bx + 3 * s, by + 70 * s);
    }
  } else {
    // No GPU rows — show placeholder
    ctx.fillStyle = '#333355';
    ctx.font = `${2 * s}px monospace`;
    ctx.fillText('GPU  N/A', bx + 2 * s, by + (13 + 2 * 14 + 5) * s);
    ctx.fillText('VRAM N/A', bx + 2 * s, by + (13 + 3 * 14 + 5) * s);
  }

  // Ollama section
  const ollamaY = by + 74 * s;
  ctx.fillStyle = '#2A2A44';
  ctx.fillRect(bx + 1 * s, ollamaY - 3 * s, pw - 2 * s, 1 * s);

  if (hw.ollama.busy && hw.ollama.models.length > 0) {
    // Blink when model is loaded
    const visible = frame % 6 < 4;
    if (visible) {
      ctx.fillStyle = '#FFAA00';
      ctx.font = `${2 * s}px monospace`;
      const modelName = hw.ollama.models[0].substring(0, 16);
      ctx.fillText(modelName, bx + 2 * s, ollamaY + 4 * s);
    }
    // Busy indicator dot
    ctx.fillStyle = frame % 4 < 2 ? '#FFAA00' : '#886600';
    ctx.fillRect(bx + pw - 6 * s, ollamaY, 3 * s, 3 * s);
  } else {
    ctx.fillStyle = '#333355';
    ctx.font = `${2 * s}px monospace`;
    ctx.fillText('OLLAMA IDLE', bx + 2 * s, ollamaY + 4 * s);
  }
}

// ─── UTILS ────────────────────────────────────────────────────────────────────

function lighten(hex: string, amount: number): string {
  const n = parseInt(hex.replace('#', ''), 16);
  const r = Math.min(255, ((n >> 16) & 0xFF) + amount);
  const g = Math.min(255, ((n >> 8) & 0xFF) + amount);
  const b = Math.min(255, (n & 0xFF) + amount);
  return `rgb(${r},${g},${b})`;
}

function darken(hex: string, amount: number): string {
  return lighten(hex, -amount);
}

export const SPRITE_SIZE = { w: CHAR_W, h: CHAR_H, scale: SCALE };
