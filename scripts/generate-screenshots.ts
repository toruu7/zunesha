import fs from 'node:fs';
import path from 'node:path';
import { createCanvas } from '/tmp/node_modules/@napi-rs/canvas';

import {
  drawCharacter, drawDesk,
  drawHelm, drawJollyRogerFlag, drawTreasureChest, drawRumBarrels,
  drawShipCannon, drawSunnyMast, drawMikanTrees, drawAnchorAndRope,
  SPRITE_SIZE,
} from '../web/playground/src/sprites';
import { drawBubble } from '../web/playground/src/bubbles';

const S = SPRITE_SIZE.scale; // 3
const CANVAS_W = 360;
const CANVAS_H = 280;
const W = CANVAS_W * S; // 1080
const H = CANVAS_H * S; // 840

function renderSunnyCanvas(frame: number = 4) {
  const canvas = createCanvas(W, H);
  const ctx = canvas.getContext('2d');
  ctx.imageSmoothingEnabled = false;

  // 1. SKY & CLOUDS
  ctx.fillStyle = '#38BDF8';
  ctx.fillRect(0, 0, W, 14 * S);
  ctx.fillStyle = 'rgba(255, 255, 255, 0.85)';
  ctx.fillRect(24 * S, 3 * S, 28 * S, 5 * S);
  ctx.fillRect(28 * S, 1 * S, 20 * S, 3 * S);
  ctx.fillRect(240 * S, 4 * S, 32 * S, 5 * S);
  ctx.fillRect(246 * S, 2 * S, 20 * S, 3 * S);

  // 2. OPEN SEA WITH ROLLING WAVES
  ctx.fillStyle = '#0284C7';
  ctx.fillRect(0, 13 * S, W, 11 * S);
  ctx.fillStyle = '#BAE6FD';
  for (let i = 0; i < 12; i++) {
    const waveX = ((i * 32 + (frame * 2)) % 360) * S;
    ctx.fillRect(waveX, 16 * S + (i % 2) * 2 * S, 14 * S, 2 * S);
    ctx.fillRect(waveX + 2 * S, 15 * S + (i % 2) * 2 * S, 10 * S, 1 * S);
  }

  // 3. THOUSAND SUNNY SHIP RAILING
  ctx.fillStyle = '#78350F';
  ctx.fillRect(0, 21 * S, W, 4 * S);
  ctx.fillStyle = '#B45309';
  ctx.fillRect(0, 20 * S, W, 2 * S);
  ctx.fillStyle = '#F59E0B';
  ctx.fillRect(0, 20 * S, W, 1 * S);
  ctx.fillStyle = '#92400E';
  for (let p = 0; p < 18; p++) {
    ctx.fillRect((p * 20 + 8) * S, 21 * S, 3 * S, 4 * S);
  }

  // 4. THOUSAND SUNNY WOODEN DECK
  ctx.fillStyle = '#B45309';
  ctx.fillRect(0, 24 * S, W, H - 24 * S);
  for (let row = 0; row < 28; row++) {
    const py = 25 * S + row * 9 * S;
    ctx.fillStyle = row % 2 === 0 ? '#92400E' : '#B45309';
    ctx.fillRect(0, py, W, 8 * S);
    ctx.fillStyle = '#451A03';
    ctx.fillRect(0, py + 8 * S, W, 1 * S);
  }
  ctx.fillStyle = '#451A03';
  ctx.globalAlpha = 0.25;
  for (let col = 0; col < 12; col++) {
    ctx.fillRect(col * 30 * S + 10 * S, 25 * S, 1 * S, H);
  }
  ctx.globalAlpha = 1;

  // 5. THOUSAND SUNNY BRIDGE & PROPS
  drawSunnyMast(ctx, 160, 5);
  drawJollyRogerFlag(ctx, 120, 3, frame);
  drawHelm(ctx, 164, 18);

  drawShipCannon(ctx, 6, 26);
  drawRumBarrels(ctx, 6, 110);
  drawTreasureChest(ctx, 10, 210);

  drawMikanTrees(ctx, 332, 26);
  drawAnchorAndRope(ctx, 332, 110);
  drawRumBarrels(ctx, 332, 210);

  // Banner
  ctx.fillStyle = '#7F1D1D';
  ctx.fillRect(125 * S, 3 * S, 110 * S, 10 * S);
  ctx.fillStyle = '#F59E0B';
  ctx.fillRect(125 * S, 3 * S, 110 * S, 1 * S);
  ctx.fillRect(125 * S, 12 * S, 110 * S, 1 * S);
  ctx.fillStyle = '#FEF08A';
  ctx.font = `bold ${3.8 * S}px monospace`;
  ctx.textAlign = 'center';
  ctx.fillText('THOUSAND SUNNY', 180 * S, 10 * S);
  ctx.textAlign = 'start';

  // 6. THE 5 STRAW HAT PIRATES AT THEIR BATTLE STATIONS
  const crew = [
    {
      id: 'luffy',
      name: 'Monkey D. Luffy',
      label: 'LUFFY',
      deskX: 168,
      deskY: 48,
      color: '#E74C3C',
      secondary: '#24488A',
      state: 'working' as const,
      bubble: "I'm gonna be King of the Pirates! Direct implementation running...",
    },
    {
      id: 'zoro',
      name: 'Roronoa Zoro',
      label: 'ZORO',
      deskX: 60,
      deskY: 65,
      color: '#27AE60',
      secondary: '#143820',
      state: 'working' as const,
      bubble: 'Zero compiler warnings. Slashing technical debt.',
    },
    {
      id: 'nami',
      name: 'Nami',
      label: 'NAMI',
      deskX: 268,
      deskY: 65,
      color: '#E67E22',
      secondary: '#2980B9',
      state: 'working' as const,
      bubble: 'Mapping out the task DAG and route interfaces.',
    },
    {
      id: 'usopp',
      name: 'Usopp',
      label: 'USOPP',
      deskX: 80,
      deskY: 155,
      color: '#8B5A2B',
      secondary: '#556B2F',
      state: 'idle' as const,
      bubble: 'Edge cases scouted! Sniper tests primed.',
    },
    {
      id: 'sanji',
      name: 'Sanji',
      label: 'SANJI',
      deskX: 248,
      deskY: 155,
      color: '#2C3E50',
      secondary: '#F1C40F',
      state: 'idle' as const,
      bubble: 'Serving polished UI and immaculate diffs.',
    },
  ];

  for (const pirate of crew) {
    drawDesk(ctx, pirate.deskX, pirate.deskY, pirate.color, pirate.label, pirate.id);
    drawCharacter(
      ctx,
      pirate.deskX,
      pirate.deskY,
      pirate.color,
      pirate.secondary,
      pirate.state,
      frame % 4,
      pirate.id
    );

    // Status dot
    const dotColor = pirate.state === 'working' ? '#00DD00' : '#888888';
    ctx.fillStyle = dotColor;
    ctx.beginPath();
    ctx.arc((pirate.deskX + 16) * S + 2 * S, (pirate.deskY + 2) * S, 2 * S, 0, Math.PI * 2);
    ctx.fill();

    // Speech bubble
    if (pirate.bubble) {
      drawBubble(ctx, pirate.deskX, pirate.deskY, pirate.bubble, pirate.state === 'working' ? 'info' : 'success');
    }
  }

  // 7. STATUS BAR
  const barH = 24 * S;
  const barY = H - barH;
  ctx.fillStyle = 'rgba(20, 20, 30, 0.94)';
  ctx.fillRect(0, barY, W, barH);
  ctx.fillStyle = '#F59E0B';
  ctx.fillRect(0, barY, W, 1);

  ctx.fillStyle = '#E2E8F0';
  ctx.font = `600 ${3.2 * S}px system-ui, -apple-system, sans-serif`;
  ctx.fillText('Straw Hat Pirates · 5 crew aboard the Thousand Sunny · 3 active missions', 12 * S, barY + 15 * S);

  return canvas;
}

function renderFullPlaygroundUI() {
  const UI_W = 1440;
  const UI_H = 920;
  const canvas = createCanvas(UI_W, UI_H);
  const ctx = canvas.getContext('2d');

  // Background
  ctx.fillStyle = '#0F172A';
  ctx.fillRect(0, 0, UI_W, UI_H);

  // 1. TOPBAR (56px)
  ctx.fillStyle = '#0B0F19';
  ctx.fillRect(0, 0, UI_W, 56);
  ctx.fillStyle = '#1E293B';
  ctx.fillRect(0, 55, UI_W, 1);

  // Brand
  ctx.font = '22px system-ui';
  ctx.fillText('🏴‍☠️', 20, 36);
  ctx.fillStyle = '#F59E0B';
  ctx.font = 'bold 15px monospace';
  ctx.fillText('THOUSAND SUNNY', 52, 28);
  ctx.fillStyle = '#94A3B8';
  ctx.font = '10px monospace';
  ctx.fillText('STRAW HAT HQ', 52, 42);

  // Project badge
  ctx.fillStyle = '#1E293B';
  ctx.fillRect(230, 14, 260, 28);
  ctx.strokeStyle = '#334155';
  ctx.lineWidth = 1;
  ctx.strokeRect(230, 14, 260, 28);
  ctx.fillStyle = '#38BDF8';
  ctx.font = 'bold 12px monospace';
  ctx.fillText('zunesha', 242, 32);
  ctx.fillStyle = '#64748B';
  ctx.font = '11px monospace';
  ctx.fillText('/mnt/f/khek/zunesha', 305, 32);

  // Status badges
  ctx.fillStyle = '#22C55E';
  ctx.beginPath();
  ctx.arc(520, 28, 4, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = '#E2E8F0';
  ctx.font = '12px system-ui';
  ctx.fillText('live · 3 working · 2 ready', 532, 32);

  // Top right link
  ctx.fillStyle = '#94A3B8';
  ctx.font = '12px system-ui';
  ctx.fillText('← Kanban & DAG Board', UI_W - 190, 33);

  // 2. LEFT RAIL (320px)
  const railW = 320;
  ctx.fillStyle = '#0F172A';
  ctx.fillRect(0, 56, railW, UI_H - 56);
  ctx.fillStyle = '#1E293B';
  ctx.fillRect(railW, 56, 1, UI_H - 56);

  // Rail Header
  ctx.fillStyle = '#F8FAFC';
  ctx.font = 'bold 16px system-ui';
  ctx.fillText('Straw Hat Crew HQ', 20, 92);
  ctx.fillStyle = '#059669';
  ctx.fillRect(railW - 75, 78, 55, 20);
  ctx.fillStyle = '#A7F3D0';
  ctx.font = 'bold 10px monospace';
  ctx.fillText('LIVE', railW - 60, 92);

  // Plan box
  ctx.fillStyle = '#1E293B';
  ctx.fillRect(16, 116, railW - 32, 140);
  ctx.strokeStyle = '#334155';
  ctx.strokeRect(16, 116, railW - 32, 140);

  ctx.fillStyle = '#94A3B8';
  ctx.font = '12px system-ui';
  ctx.fillText('Plan a mission with Nami', 28, 140);

  ctx.fillStyle = '#0F172A';
  ctx.fillRect(28, 150, railW - 56, 50);
  ctx.fillStyle = '#475569';
  ctx.font = '12px system-ui';
  ctx.fillText('Tell Nami & the crew what feature to build…', 38, 175);

  ctx.fillStyle = '#D97706';
  ctx.fillRect(28, 210, 70, 32);
  ctx.fillStyle = '#FFFFFF';
  ctx.font = 'bold 12px system-ui';
  ctx.fillText('Plan', 50, 230);

  // Task list header
  ctx.fillStyle = '#94A3B8';
  ctx.font = 'bold 12px system-ui';
  ctx.fillText('ACTIVE CREW MISSIONS', 20, 286);

  // Tasks in rail
  const tasks = [
    { id: '#0', title: 'Feature Driver: Implement Core', crew: 'Luffy 👒', status: 'IN_PROGRESS', bg: '#1E293B' },
    { id: '#1', title: 'Refactor & Zero Warnings', crew: 'Zoro ⚔️', status: 'IN_PROGRESS', bg: '#1E293B' },
    { id: '#2', title: 'Design Schema & Routes', crew: 'Nami 🗺️', status: 'IN_PROGRESS', bg: '#1E293B' },
    { id: '#3', title: 'Scout Edge Cases & Tests', crew: 'Usopp 🎯', status: 'TODO', bg: '#182030' },
    { id: '#4', title: 'UI Aesthetics & Polish Diff', crew: 'Sanji 🍳', status: 'TODO', bg: '#182030' },
  ];

  let taskY = 300;
  for (const t of tasks) {
    ctx.fillStyle = t.bg;
    ctx.fillRect(16, taskY, railW - 32, 54);
    ctx.strokeStyle = '#334155';
    ctx.strokeRect(16, taskY, railW - 32, 54);

    ctx.fillStyle = '#F8FAFC';
    ctx.font = 'bold 12px system-ui';
    ctx.fillText(`${t.id} ${t.title}`, 26, taskY + 22);

    ctx.fillStyle = '#F59E0B';
    ctx.font = '11px system-ui';
    ctx.fillText(t.crew, 26, taskY + 40);

    ctx.fillStyle = t.status === 'IN_PROGRESS' ? '#059669' : '#475569';
    ctx.fillRect(railW - 105, taskY + 26, 75, 18);
    ctx.fillStyle = '#FFFFFF';
    ctx.font = 'bold 9px monospace';
    ctx.fillText(t.status, railW - 98, taskY + 39);

    taskY += 62;
  }

  // 3. CENTER: THOUSAND SUNNY CANVAS (scale down slightly to fit 820x638)
  const sunnyCanvas = renderSunnyCanvas(4);
  const targetW = 800;
  const targetH = 622;
  const deckX = railW + 20;
  const deckY = 76;

  ctx.strokeStyle = '#334155';
  ctx.lineWidth = 2;
  ctx.strokeRect(deckX - 2, deckY - 2, targetW + 4, targetH + 4);
  ctx.drawImage(sunnyCanvas, deckX, deckY, targetW, targetH);

  // 4. LIVE TRANSCRIPT DOCK (below canvas)
  const dockY = deckY + targetH + 16;
  const dockH = UI_H - dockY - 16;
  ctx.fillStyle = '#0B0F19';
  ctx.fillRect(deckX, dockY, targetW, dockH);
  ctx.strokeStyle = '#1E293B';
  ctx.strokeRect(deckX, dockY, targetW, dockH);

  ctx.fillStyle = '#1E293B';
  ctx.fillRect(deckX, dockY, targetW, 26);
  ctx.fillStyle = '#F59E0B';
  ctx.font = 'bold 11px monospace';
  ctx.fillText('LIVE TRANSCRIPT: Monkey D. Luffy (Captain · Feature Driver)', deckX + 12, dockY + 18);

  ctx.fillStyle = '#38BDF8';
  ctx.font = '11px monospace';
  ctx.fillText('[worker] task claimed by luffy | persona: Straw Hat Captain', deckX + 12, dockY + 45);
  ctx.fillStyle = '#94A3B8';
  ctx.fillText('[opencode] reading codebase map and workspace diff markers...', deckX + 12, dockY + 65);
  ctx.fillStyle = '#4ADE80';
  ctx.fillText('[opencode] workspace code generated cleanly with 0 exit code', deckX + 12, dockY + 85);
  ctx.fillStyle = '#FBBF24';
  ctx.fillText('[eval] cargo test --workspace: 258 passed, 0 failed', deckX + 12, dockY + 105);

  // 5. RIGHT INSPECTOR (260px)
  const inspX = deckX + targetW + 20;
  const inspW = UI_W - inspX - 20;
  ctx.fillStyle = '#0B0F19';
  ctx.fillRect(inspX, 76, inspW, UI_H - 92);
  ctx.strokeStyle = '#1E293B';
  ctx.strokeRect(inspX, 76, inspW, UI_H - 92);

  ctx.fillStyle = '#1E293B';
  ctx.fillRect(inspX, 76, inspW, 36);
  ctx.fillStyle = '#F8FAFC';
  ctx.font = 'bold 13px system-ui';
  ctx.fillText('Crew Dossier: Luffy', inspX + 14, 100);

  ctx.fillStyle = '#E2E8F0';
  ctx.font = 'bold 14px system-ui';
  ctx.fillText('Monkey D. Luffy', inspX + 14, 140);
  ctx.fillStyle = '#F59E0B';
  ctx.font = '12px system-ui';
  ctx.fillText('Captain · Feature Driver', inspX + 14, 160);

  ctx.fillStyle = '#94A3B8';
  ctx.font = '11px system-ui';
  ctx.fillText('Status: Active on Mission #0', inspX + 14, 190);
  ctx.fillText('Model: opencode/deepseek-v4.1', inspX + 14, 210);
  ctx.fillText('Provider: OpenCode (JSON stream)', inspX + 14, 230);

  ctx.fillStyle = '#334155';
  ctx.fillRect(inspX + 14, 250, inspW - 28, 1);

  ctx.fillStyle = '#F8FAFC';
  ctx.font = 'bold 12px system-ui';
  ctx.fillText('Character Directive', inspX + 14, 275);
  ctx.fillStyle = '#94A3B8';
  ctx.font = '11px system-ui';
  ctx.fillText('High-agency, pragmatic', inspX + 14, 298);
  ctx.fillText('implementation of new', inspX + 14, 316);
  ctx.fillText('features without analysis', inspX + 14, 334);
  ctx.fillText('paralysis. Writes working', inspX + 14, 352);
  ctx.fillText('code directly.', inspX + 14, 370);

  return canvas;
}

// Generate and write
const sunny = renderSunnyCanvas(4);
const sunnyBuf = sunny.toBuffer('image/png');
fs.writeFileSync(path.resolve('docs/src/images/thousand-sunny-deck.png'), sunnyBuf);
fs.writeFileSync(path.resolve('web/public/playground/thousand-sunny-deck.png'), sunnyBuf);
console.log('Saved thousand-sunny-deck.png, size:', sunnyBuf.length);

const overview = renderFullPlaygroundUI();
const overviewBuf = overview.toBuffer('image/png');
fs.writeFileSync(path.resolve('docs/src/images/thousand-sunny-playground.png'), overviewBuf);
fs.writeFileSync(path.resolve('web/public/playground/thousand-sunny-playground.png'), overviewBuf);
console.log('Saved thousand-sunny-playground.png, size:', overviewBuf.length);
