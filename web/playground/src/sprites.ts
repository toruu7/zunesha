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

  const rawId = (agentId ?? '').toLowerCase();
  const isLuffy = rawId.includes('luffy');
  const isZoro = rawId.includes('zoro');
  const isNami = rawId.includes('nami');
  const isUsopp = rawId.includes('usopp');
  const isSanji = rawId.includes('sanji');
  const isChopper = rawId.includes('chopper');
  const isRobin = rawId.includes('robin');
  const isFranky = rawId.includes('franky');
  const isBrook = rawId.includes('brook');
  const isJinbe = rawId.includes('jinbe');

  // Resolve colors — use appearance overrides if provided, or Straw Hat character defaults
  const skinColor  = appearance?.skinTone
    ?? (isLuffy ? '#F2C290'
      : isZoro ? '#E2AF7C'
      : isNami ? '#F7D0B2'
      : isUsopp ? '#C89260'
      : isSanji ? '#F4C8A0'
      : isChopper ? '#8D6E63'
      : isRobin ? '#F5D6BA'
      : isFranky ? '#F2C290'
      : isBrook ? '#F5F6FA'
      : isJinbe ? '#3498DB'
      : '#D4A870');

  const outfitColor = appearance?.outfitColor
    ?? (isLuffy ? '#D92020'   // Red vest
      : isZoro ? '#1E5830'    // Deep green coat
      : isNami ? '#2A82BE'    // Blue & white
      : isUsopp ? '#8B5A2B'   // Brown dungarees
      : isSanji ? '#181822'   // Midnight suit
      : isChopper ? '#E84393' // Cherry pink
      : isRobin ? '#6C3483'   // Royal purple sarong
      : isFranky ? '#D63031'  // Hawaiian flame red
      : isBrook ? '#2D3436'   // Black gentleman coat
      : isJinbe ? '#E67E22'   // Sun orange kimono
      : color);

  const hairColor  = appearance?.hairColor
    ?? (isLuffy ? '#111111'
      : isZoro ? '#3CB371'  // Marimo green
      : isNami ? '#E65100'  // Vibrant orange
      : isUsopp ? '#1A1A1A' // Dark curly
      : isSanji ? '#F4D03F' // Golden blonde
      : isChopper ? '#4E342E'
      : isRobin ? '#111827' // Glossy black
      : isFranky ? '#00CEC9' // Bright cyan pompadour
      : isBrook ? '#111111' // Afro black
      : isJinbe ? '#1E272E'
      : agentId === 'analista' ? '#2A4A2A'
      : agentId === 'bibliotecario' ? '#3A2060'
      : agentId === 'rafael' ? '#A05010'
      : '#2A1A0A');

  const accessory  = appearance?.accessory
    ?? (isLuffy ? 'strawhat'
      : isZoro ? 'earrings'
      : isNami ? 'logpose'
      : isUsopp ? 'goggles'
      : isSanji ? 'cigarette'
      : isChopper ? 'chopper_hat'
      : isRobin ? 'robin_shades'
      : isFranky ? 'cyborg_shoulders'
      : isBrook ? 'soul_cane'
      : isJinbe ? 'jinbe_tusks'
      : agentId === 'main' ? 'headset'
      : agentId === 'analista' ? 'glasses'
      : 'none');

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
  if (isLuffy) {
    // Blue shorts cuff, then bare skin ankles and sandals
    ctx.fillStyle = skinColor;
    ctx.fillRect(cx + 5 * s, by + 20 * s, 3 * s, 2 * s);
    ctx.fillRect(cx + 9 * s, by + 20 * s, 3 * s, 2 * s);
    ctx.fillStyle = '#8B4513'; // Straw sandals
    ctx.fillRect(cx + 5 * s, by + 21 * s, 3 * s, 1 * s);
    ctx.fillRect(cx + 9 * s, by + 21 * s, 3 * s, 1 * s);
  }

  // Torso
  ctx.fillStyle = outfitColor;
  ctx.fillRect(cx + 4 * s, by + 10 * s, 9 * s, 8 * s);

  if (isLuffy) {
    // Open red vest with visible chest
    ctx.fillStyle = skinColor;
    ctx.fillRect(cx + 8 * s, by + 10 * s, 1 * s, 6 * s);
    // Yellow waist sash
    ctx.fillStyle = '#F1C40F';
    ctx.fillRect(cx + 4 * s, by + 16 * s, 9 * s, 2 * s);
  } else if (isZoro) {
    // Olive green haramaki bellyband
    ctx.fillStyle = '#7CB342';
    ctx.fillRect(cx + 4 * s, by + 15 * s, 9 * s, 3 * s);
  } else if (isNami) {
    // White stripe on blue top
    ctx.fillStyle = '#FFFFFF';
    ctx.fillRect(cx + 4 * s, by + 12 * s, 9 * s, 2 * s);
  } else if (isUsopp) {
    // Tool strap across torso
    ctx.fillStyle = '#E0E0E0';
    ctx.fillRect(cx + 5 * s, by + 10 * s, 2 * s, 8 * s);
  } else if (isSanji) {
    // Blue dress shirt and black tie
    ctx.fillStyle = '#90CAF9';
    ctx.fillRect(cx + 7 * s, by + 10 * s, 3 * s, 3 * s);
    ctx.fillStyle = '#0F172A';
    ctx.fillRect(cx + 8 * s, by + 11 * s, 1 * s, 5 * s);
  } else if (isChopper) {
    // Medical blue backpack strap & shorts
    ctx.fillStyle = '#00CEC9';
    ctx.fillRect(cx + 5 * s, by + 10 * s, 1 * s, 8 * s);
    ctx.fillRect(cx + 11 * s, by + 10 * s, 1 * s, 8 * s);
    ctx.fillStyle = '#881337';
    ctx.fillRect(cx + 4 * s, by + 16 * s, 9 * s, 2 * s);
  } else if (isRobin) {
    // Elegant deep purple sarong and golden buckle
    ctx.fillStyle = '#4A235A';
    ctx.fillRect(cx + 6 * s, by + 10 * s, 5 * s, 4 * s);
    ctx.fillStyle = '#F59E0B';
    ctx.fillRect(cx + 8 * s, by + 15 * s, 1 * s, 2 * s);
  } else if (isFranky) {
    // Red Hawaiian shirt with yellow palm stars & thick neck chain
    ctx.fillStyle = '#F1C40F';
    ctx.fillRect(cx + 6 * s, by + 12 * s, 2 * s, 2 * s);
    ctx.fillRect(cx + 10 * s, by + 14 * s, 2 * s, 2 * s);
    ctx.fillStyle = '#F59E0B';
    ctx.fillRect(cx + 6 * s, by + 10 * s, 5 * s, 1 * s);
  } else if (isBrook) {
    // Fluffy golden cravat & soul jewel
    ctx.fillStyle = '#F39C12';
    ctx.fillRect(cx + 7 * s, by + 10 * s, 3 * s, 5 * s);
    ctx.fillStyle = '#2ECC71';
    ctx.fillRect(cx + 8 * s, by + 11 * s, 1 * s, 2 * s);
  } else if (isJinbe) {
    // Sun Pirates tattoo on chest & sun crest kimono
    ctx.fillStyle = '#C0392B';
    ctx.fillRect(cx + 7 * s, by + 11 * s, 3 * s, 3 * s);
    ctx.fillStyle = '#111827';
    ctx.fillRect(cx + 4 * s, by + 16 * s, 9 * s, 2 * s);
  } else {
    // Collar/shirt detail
    ctx.fillStyle = lighten(outfitColor, 30);
    ctx.fillRect(cx + 7 * s, by + 10 * s, 3 * s, 2 * s);
  }

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
  if (isNami) {
    // Long flowing orange hair down shoulders
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 3 * s);
    ctx.fillRect(cx + 3 * s, by + 2 * s, 2 * s, 14 * s);
    ctx.fillRect(cx + 12 * s, by + 2 * s, 2 * s, 14 * s);
  } else if (isSanji) {
    // Swooped blonde hair covering right eye
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 4 * s);
    ctx.fillRect(cx + 4 * s, by + 2 * s, 1 * s, 6 * s);
    ctx.fillRect(cx + 9 * s, by + 2 * s, 4 * s, 7 * s); // Covers right eye
  } else if (isRobin) {
    // Long straight glossy black hair
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 3 * s);
    ctx.fillRect(cx + 3 * s, by + 2 * s, 2 * s, 15 * s);
    ctx.fillRect(cx + 12 * s, by + 2 * s, 2 * s, 15 * s);
  } else if (isFranky) {
    // Cyan sweeping pompadour
    ctx.fillRect(cx + 5 * s, by - 3 * s, 6 * s, 4 * s);
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 3 * s);
  } else if (isBrook) {
    // Huge black spherical afro
    ctx.fillRect(cx + 2 * s, by - 6 * s, 13 * s, 9 * s);
    ctx.fillRect(cx + 1 * s, by - 4 * s, 15 * s, 6 * s);
  } else if (isJinbe) {
    // Samurai topknot and sideburn tufts
    ctx.fillRect(cx + 7 * s, by - 3 * s, 3 * s, 4 * s);
    ctx.fillRect(cx + 3 * s, by + 3 * s, 2 * s, 5 * s);
    ctx.fillRect(cx + 12 * s, by + 3 * s, 2 * s, 5 * s);
  } else {
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 3 * s);
    ctx.fillRect(cx + 4 * s, by + 2 * s, 1 * s, 5 * s);
    ctx.fillRect(cx + 12 * s, by + 2 * s, 1 * s, 5 * s);
  }

  // Accessories
  if (accessory === 'strawhat') {
    // Luffy's iconic Straw Hat
    ctx.fillStyle = '#F1C40F'; // Straw yellow brim
    ctx.fillRect(cx + 1 * s, by + 1 * s, 15 * s, 2 * s);
    ctx.fillStyle = '#C0392B'; // Red ribbon band
    ctx.fillRect(cx + 4 * s, by - 1 * s, 9 * s, 2 * s);
    ctx.fillStyle = '#F1C40F'; // Top straw dome
    ctx.fillRect(cx + 5 * s, by - 4 * s, 7 * s, 3 * s);
  } else if (accessory === 'earrings') {
    // Zoro's 3 gold earrings on left ear
    ctx.fillStyle = '#FFD700';
    ctx.fillRect(cx + 3 * s, by + 5 * s, 1 * s, 1 * s);
    ctx.fillRect(cx + 3 * s, by + 7 * s, 1 * s, 1 * s);
    ctx.fillRect(cx + 3 * s, by + 9 * s, 1 * s, 1 * s);
  } else if (accessory === 'logpose') {
    // Nami's Log Pose compass on wrist
    ctx.fillStyle = '#FFD700';
    ctx.fillRect(cx + 2 * s, by + 16 * s, 2 * s, 2 * s);
    ctx.fillStyle = '#38BDF8';
    ctx.fillRect(cx + 2 * s, by + 16 * s, 1 * s, 1 * s);
  } else if (accessory === 'goggles') {
    // Usopp's bandana & goggles
    ctx.fillStyle = '#556B2F'; // Olive bandana
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 2 * s);
    ctx.fillStyle = '#D97706'; // Brass goggle frame
    ctx.fillRect(cx + 5 * s, by + 1 * s, 3 * s, 2 * s);
    ctx.fillRect(cx + 9 * s, by + 1 * s, 3 * s, 2 * s);
    ctx.fillStyle = '#60A5FA'; // Blue lens
    ctx.fillRect(cx + 6 * s, by + 1 * s, 1 * s, 1 * s);
    ctx.fillRect(cx + 10 * s, by + 1 * s, 1 * s, 1 * s);
    // Long nose
    ctx.fillStyle = skinColor;
    ctx.fillRect(cx + 12 * s, by + 6 * s, 3 * s, 2 * s);
  } else if (accessory === 'cigarette') {
    // Sanji's cigarette
    ctx.fillStyle = '#FFFFFF'; // Cigarette stick
    ctx.fillRect(cx + 10 * s, by + 8 * s, 3 * s, 1 * s);
    ctx.fillStyle = '#EF4444'; // Glowing ember
    ctx.fillRect(cx + 13 * s, by + 8 * s, 1 * s, 1 * s);
    // Rising smoke
    ctx.fillStyle = 'rgba(220, 220, 220, 0.7)';
    ctx.fillRect(cx + 14 * s, by + 5 * s, 1 * s, 2 * s);
  } else if (accessory === 'chopper_hat') {
    // Chopper's tall pink hat with medical cross & antlers
    ctx.fillStyle = '#E84393';
    ctx.fillRect(cx + 2 * s, by + 1 * s, 13 * s, 2 * s); // brim
    ctx.fillRect(cx + 4 * s, by - 4 * s, 9 * s, 5 * s);  // crown
    // White medical cross
    ctx.fillStyle = '#FFFFFF';
    ctx.fillRect(cx + 8 * s, by - 3 * s, 1 * s, 3 * s);
    ctx.fillRect(cx + 7 * s, by - 2 * s, 3 * s, 1 * s);
    // Antlers
    ctx.fillStyle = '#5D4037';
    ctx.fillRect(cx + 1 * s, by - 3 * s, 2 * s, 2 * s);
    ctx.fillRect(cx + 2 * s, by - 6 * s, 1 * s, 4 * s);
    ctx.fillRect(cx + 14 * s, by - 3 * s, 2 * s, 2 * s);
    ctx.fillRect(cx + 14 * s, by - 6 * s, 1 * s, 4 * s);
    // Blue reindeer nose
    ctx.fillStyle = '#00CEC9';
    ctx.fillRect(cx + 8 * s, by + 6 * s, 2 * s, 2 * s);
  } else if (accessory === 'robin_shades') {
    // Chic orange sunglasses resting on head
    ctx.fillStyle = '#111827';
    ctx.fillRect(cx + 4 * s, by + 1 * s, 9 * s, 1 * s);
    ctx.fillStyle = '#F59E0B';
    ctx.fillRect(cx + 5 * s, by, 3 * s, 2 * s);
    ctx.fillRect(cx + 9 * s, by, 3 * s, 2 * s);
  } else if (accessory === 'cyborg_shoulders') {
    // Massive steel cyborg shoulders and steel nose
    ctx.fillStyle = '#BDC3C7';
    ctx.fillRect(cx + 1 * s, by + 10 * s, 3 * s, 3 * s);
    ctx.fillRect(cx + 13 * s, by + 10 * s, 3 * s, 3 * s);
    ctx.fillStyle = '#7F8C8D';
    ctx.fillRect(cx + 8 * s, by + 5 * s, 2 * s, 2 * s);
  } else if (accessory === 'soul_cane') {
    // Purple walking cane / sword
    ctx.fillStyle = '#8E44AD';
    ctx.fillRect(cx + 14 * s, by + 11 * s, 1 * s, 11 * s);
    ctx.fillRect(cx + 13 * s, by + 10 * s, 2 * s, 1 * s);
  } else if (accessory === 'jinbe_tusks') {
    // White tusks curving up from lower jaw
    ctx.fillStyle = '#FFFFFF';
    ctx.fillRect(cx + 5 * s, by + 7 * s, 1 * s, 2 * s);
    ctx.fillRect(cx + 11 * s, by + 7 * s, 1 * s, 2 * s);
    // Lightning scar over left eye
    ctx.fillStyle = '#C0392B';
    ctx.fillRect(cx + 6 * s, by + 3 * s, 1 * s, 3 * s);
  } else if (accessory === 'headset') {
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
    if (!isSanji) ctx.fillRect(cx + 10 * s, by + 5 * s, 2 * s, 1 * s);
  } else {
    ctx.fillStyle = '#111';
    ctx.fillRect(cx + 6 * s, by + 5 * s, 2 * s, 2 * s);
    if (!isSanji) ctx.fillRect(cx + 10 * s, by + 5 * s, 2 * s, 2 * s);
    ctx.fillStyle = '#FFF';
    ctx.fillRect(cx + 7 * s, by + 5 * s, 1 * s, 1 * s);
    if (!isSanji) ctx.fillRect(cx + 11 * s, by + 5 * s, 1 * s, 1 * s);
  }

  // Scars & Details
  if (isLuffy) {
    // Under eye stitch scar
    ctx.fillStyle = '#B91C1C';
    ctx.fillRect(cx + 6 * s, by + 7 * s, 2 * s, 1 * s);
    ctx.fillRect(cx + 6 * s, by + 8 * s, 1 * s, 1 * s);
  } else if (isZoro) {
    // Katana scar slicing through left eye
    ctx.fillStyle = '#7F1D1D';
    ctx.fillRect(cx + 7 * s, by + 4 * s, 1 * s, 4 * s);
  } else if (isSanji) {
    // Curly eyebrow tip on visible left eye
    ctx.fillStyle = '#D97706';
    ctx.fillRect(cx + 5 * s, by + 4 * s, 1 * s, 1 * s);
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
  const raw = (agentId ?? '').toLowerCase();
  if (raw.includes('luffy')) drawLuffyDesk(ctx, dx, dy);
  else if (raw.includes('zoro')) drawZoroDesk(ctx, dx, dy);
  else if (raw.includes('nami')) drawNamiDesk(ctx, dx, dy);
  else if (raw.includes('usopp')) drawUsoppDesk(ctx, dx, dy);
  else if (raw.includes('sanji')) drawSanjiDesk(ctx, dx, dy);
  else drawLuffyDesk(ctx, dx, dy);

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

function drawLuffyDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Center: Pirate terminal monitor
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 2 * s, dy + 1 * s, 11 * s, 7 * s);
  ctx.fillStyle = '#1A0808';
  ctx.fillRect(dx + 3 * s, dy + 2 * s, 9 * s, 5 * s);

  // Skull icon on screen (Jolly Roger)
  ctx.fillStyle = '#FFFFFF';
  ctx.fillRect(dx + 6 * s, dy + 3 * s, 3 * s, 2 * s);
  ctx.fillRect(dx + 7 * s, dy + 5 * s, 1 * s, 1 * s);
  ctx.fillStyle = '#1A0808';
  ctx.fillRect(dx + 6 * s, dy + 4 * s, 1 * s, 1 * s);
  ctx.fillRect(dx + 8 * s, dy + 4 * s, 1 * s, 1 * s);
  ctx.fillStyle = '#FF4444';
  ctx.fillRect(dx + 5 * s, dy + 3 * s, 5 * s, 1 * s);

  // Monitor stand
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 7 * s, dy + 8 * s, 2 * s, 1 * s);

  // Left: Big juicy meat on bone
  ctx.fillStyle = '#F5F5DC';
  ctx.fillRect(dx - 4 * s, dy + 4 * s, 2 * s, 2 * s);
  ctx.fillRect(dx + 0 * s, dy + 4 * s, 2 * s, 2 * s);
  ctx.fillStyle = '#8B2500';
  ctx.fillRect(dx - 3 * s, dy + 3 * s, 3 * s, 4 * s);
  ctx.fillStyle = '#CD5C5C';
  ctx.fillRect(dx - 2 * s, dy + 4 * s, 2 * s, 2 * s);

  // Right: Straw Hat mug with steaming drink
  ctx.fillStyle = '#F4D03F';
  ctx.fillRect(dx + 14 * s, dy + 6 * s, 3 * s, 4 * s);
  ctx.fillStyle = '#E74C3C';
  ctx.fillRect(dx + 14 * s, dy + 7 * s, 3 * s, 1 * s);
  ctx.fillStyle = '#F4D03F';
  ctx.fillRect(dx + 17 * s, dy + 7 * s, 1 * s, 2 * s);
  ctx.fillStyle = '#FFFFFF';
  ctx.globalAlpha = 0.5;
  ctx.fillRect(dx + 15 * s, dy + 4 * s, 1 * s, 1 * s);
  ctx.fillRect(dx + 16 * s, dy + 3 * s, 1 * s, 1 * s);
  ctx.globalAlpha = 1.0;

  // Keyboard
  ctx.fillStyle = '#991B1B';
  ctx.fillRect(dx + 3 * s, dy + 9 * s, 9 * s, 2 * s);
}

function drawZoroDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Center: Slashing terminal monitor
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 2 * s, dy + 1 * s, 11 * s, 7 * s);
  ctx.fillStyle = '#05180A';
  ctx.fillRect(dx + 3 * s, dy + 2 * s, 9 * s, 5 * s);

  // Slashing green energy aura lines
  ctx.fillStyle = '#10B981';
  ctx.fillRect(dx + 4 * s, dy + 3 * s, 4 * s, 1 * s);
  ctx.fillRect(dx + 7 * s, dy + 4 * s, 4 * s, 1 * s);
  ctx.fillRect(dx + 5 * s, dy + 5 * s, 5 * s, 1 * s);

  // Monitor stand
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 7 * s, dy + 8 * s, 2 * s, 1 * s);

  // Left: 3 Katana rack on desk (Wado Ichimonji, Sandai Kitetsu, Enma)
  ctx.fillStyle = '#FAFAFA';
  ctx.fillRect(dx - 4 * s, dy + 3 * s, 4 * s, 1 * s);
  ctx.fillStyle = '#CBD5E1';
  ctx.fillRect(dx - 1 * s, dy + 3 * s, 2 * s, 1 * s);
  ctx.fillStyle = '#DC2626';
  ctx.fillRect(dx - 4 * s, dy + 5 * s, 4 * s, 1 * s);
  ctx.fillStyle = '#CBD5E1';
  ctx.fillRect(dx - 1 * s, dy + 5 * s, 2 * s, 1 * s);
  ctx.fillStyle = '#7C3AED';
  ctx.fillRect(dx - 4 * s, dy + 7 * s, 4 * s, 1 * s);
  ctx.fillStyle = '#F59E0B';
  ctx.fillRect(dx - 1 * s, dy + 7 * s, 1 * s, 1 * s);

  // Right: Sake gourd & small cup
  ctx.fillStyle = '#854D0E';
  ctx.fillRect(dx + 15 * s, dy + 6 * s, 2 * s, 3 * s);
  ctx.fillRect(dx + 15 * s, dy + 4 * s, 1 * s, 2 * s);
  ctx.fillStyle = '#DC2626';
  ctx.fillRect(dx + 14 * s, dy + 5 * s, 3 * s, 1 * s);
  ctx.fillStyle = '#FEF3C7';
  ctx.fillRect(dx + 13 * s, dy + 8 * s, 2 * s, 1 * s);

  // Keyboard
  ctx.fillStyle = '#064E3B';
  ctx.fillRect(dx + 3 * s, dy + 9 * s, 9 * s, 2 * s);
}

function drawNamiDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Center: Weather / Navigation radar monitor
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 2 * s, dy + 1 * s, 11 * s, 7 * s);
  ctx.fillStyle = '#041B2D';
  ctx.fillRect(dx + 3 * s, dy + 2 * s, 9 * s, 5 * s);

  // Radar circular rings / weather storm patterns
  ctx.fillStyle = '#06B6D4';
  ctx.fillRect(dx + 6 * s, dy + 3 * s, 3 * s, 1 * s);
  ctx.fillRect(dx + 4 * s, dy + 4 * s, 7 * s, 1 * s);
  ctx.fillRect(dx + 7 * s, dy + 5 * s, 4 * s, 1 * s);
  ctx.fillStyle = '#F59E0B';
  ctx.fillRect(dx + 7 * s, dy + 4 * s, 1 * s, 1 * s);

  // Monitor stand
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 7 * s, dy + 8 * s, 2 * s, 1 * s);

  // Left: Rolled sea charts & maps
  ctx.fillStyle = '#FEF3C7';
  ctx.fillRect(dx - 4 * s, dy + 4 * s, 5 * s, 4 * s);
  ctx.fillStyle = '#D97706';
  ctx.fillRect(dx - 3 * s, dy + 5 * s, 3 * s, 1 * s);
  ctx.fillRect(dx - 2 * s, dy + 6 * s, 2 * s, 1 * s);
  ctx.fillStyle = '#B45309';
  ctx.fillRect(dx - 4 * s, dy + 7 * s, 5 * s, 1 * s);

  // Right: Mikan (tangerine) & Beli gold coin pouch
  ctx.fillStyle = '#EA580C';
  ctx.fillRect(dx + 14 * s, dy + 5 * s, 3 * s, 3 * s);
  ctx.fillStyle = '#15803D';
  ctx.fillRect(dx + 15 * s, dy + 4 * s, 1 * s, 1 * s);
  ctx.fillStyle = '#FBBF24';
  ctx.fillRect(dx + 13 * s, dy + 8 * s, 3 * s, 2 * s);
  ctx.fillStyle = '#D97706';
  ctx.fillRect(dx + 14 * s, dy + 7 * s, 1 * s, 1 * s);

  // Keyboard
  ctx.fillStyle = '#0891B2';
  ctx.fillRect(dx + 3 * s, dy + 9 * s, 9 * s, 2 * s);
}

function drawUsoppDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Center: Blueprint / Invention gadget terminal
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 2 * s, dy + 1 * s, 11 * s, 7 * s);
  ctx.fillStyle = '#0F2618';
  ctx.fillRect(dx + 3 * s, dy + 2 * s, 9 * s, 5 * s);

  // Blueprint schematic wireframes
  ctx.fillStyle = '#34D399';
  ctx.fillRect(dx + 4 * s, dy + 3 * s, 4 * s, 1 * s);
  ctx.fillRect(dx + 4 * s, dy + 4 * s, 1 * s, 2 * s);
  ctx.fillRect(dx + 7 * s, dy + 4 * s, 4 * s, 1 * s);
  ctx.fillStyle = '#FBBF24';
  ctx.fillRect(dx + 8 * s, dy + 5 * s, 2 * s, 1 * s);

  // Monitor stand
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 7 * s, dy + 8 * s, 2 * s, 1 * s);

  // Left: Kuro Kabuto slingshot & Pop Green seeds
  ctx.fillStyle = '#1E3A1E';
  ctx.fillRect(dx - 3 * s, dy + 3 * s, 1 * s, 5 * s);
  ctx.fillStyle = '#22C55E';
  ctx.fillRect(dx - 4 * s, dy + 2 * s, 3 * s, 1 * s);
  ctx.fillStyle = '#10B981';
  ctx.fillRect(dx - 1 * s, dy + 7 * s, 2 * s, 2 * s);

  // Right: Inventor toolbox & wrench
  ctx.fillStyle = '#B91C1C';
  ctx.fillRect(dx + 14 * s, dy + 5 * s, 4 * s, 4 * s);
  ctx.fillStyle = '#94A3B8';
  ctx.fillRect(dx + 15 * s, dy + 4 * s, 2 * s, 1 * s);
  ctx.fillStyle = '#E2E8F0';
  ctx.fillRect(dx + 14 * s, dy + 7 * s, 2 * s, 1 * s);

  // Keyboard
  ctx.fillStyle = '#D97706';
  ctx.fillRect(dx + 3 * s, dy + 9 * s, 9 * s, 2 * s);
}

function drawSanjiDesk(ctx: CanvasRenderingContext2D, dx: number, dy: number): void {
  // Center: Baratie / French cuisine stylish terminal
  ctx.fillStyle = '#111';
  ctx.fillRect(dx + 2 * s, dy + 1 * s, 11 * s, 7 * s);
  ctx.fillStyle = '#0F172A';
  ctx.fillRect(dx + 3 * s, dy + 2 * s, 9 * s, 5 * s);

  // Golden cursive receipt / recipe stream
  ctx.fillStyle = '#F59E0B';
  ctx.fillRect(dx + 4 * s, dy + 3 * s, 6 * s, 1 * s);
  ctx.fillRect(dx + 4 * s, dy + 4 * s, 4 * s, 1 * s);
  ctx.fillRect(dx + 4 * s, dy + 5 * s, 5 * s, 1 * s);
  ctx.fillStyle = '#38BDF8';
  ctx.fillRect(dx + 8 * s, dy + 5 * s, 2 * s, 1 * s);

  // Monitor stand
  ctx.fillStyle = '#333';
  ctx.fillRect(dx + 7 * s, dy + 8 * s, 2 * s, 1 * s);

  // Left: Polished chef pan & santoku knife
  ctx.fillStyle = '#334155';
  ctx.fillRect(dx - 4 * s, dy + 5 * s, 4 * s, 3 * s);
  ctx.fillStyle = '#64748B';
  ctx.fillRect(dx - 5 * s, dy + 6 * s, 2 * s, 1 * s);
  ctx.fillStyle = '#F59E0B';
  ctx.fillRect(dx - 3 * s, dy + 6 * s, 2 * s, 1 * s);

  // Right: Fine porcelain espresso cup on saucer + rising steam
  ctx.fillStyle = '#E2E8F0';
  ctx.fillRect(dx + 13 * s, dy + 8 * s, 4 * s, 1 * s);
  ctx.fillStyle = '#FFFFFF';
  ctx.fillRect(dx + 14 * s, dy + 6 * s, 2 * s, 2 * s);
  ctx.fillStyle = '#1E293B';
  ctx.fillRect(dx + 14 * s, dy + 6 * s, 2 * s, 1 * s);
  ctx.fillStyle = '#FFFFFF';
  ctx.globalAlpha = 0.5;
  ctx.fillRect(dx + 15 * s, dy + 4 * s, 1 * s, 1 * s);
  ctx.fillRect(dx + 14 * s, dy + 3 * s, 1 * s, 1 * s);
  ctx.globalAlpha = 1.0;

  // Keyboard
  ctx.fillStyle = '#1E3A8A';
  ctx.fillRect(dx + 3 * s, dy + 9 * s, 9 * s, 2 * s);
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

// ─── PIRATE SHIP PROPS (Thousand Sunny Deck Scene) ───────────────────────────

export function drawHelm(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  // Wooden helm wheel post
  ctx.fillStyle = '#5A3815';
  ctx.fillRect(bx + 7 * s, by + 12 * s, 6 * s, 14 * s);
  ctx.fillStyle = '#3A2008';
  ctx.fillRect(bx + 6 * s, by + 24 * s, 8 * s, 4 * s);

  // Outer wheel ring
  ctx.fillStyle = '#8B5A2B';
  ctx.fillRect(bx + 3 * s, by + 2 * s, 14 * s, 2 * s);
  ctx.fillRect(bx + 3 * s, by + 16 * s, 14 * s, 2 * s);
  ctx.fillRect(bx + 1 * s, by + 4 * s, 2 * s, 12 * s);
  ctx.fillRect(bx + 17 * s, by + 4 * s, 2 * s, 12 * s);

  // 8 handles (spokes extending outward)
  ctx.fillStyle = '#C89260';
  ctx.fillRect(bx + 9 * s, by - 2 * s, 2 * s, 4 * s); // top
  ctx.fillRect(bx + 9 * s, by + 18 * s, 2 * s, 4 * s); // bottom
  ctx.fillRect(bx - 3 * s, by + 9 * s, 4 * s, 2 * s); // left
  ctx.fillRect(bx + 19 * s, by + 9 * s, 4 * s, 2 * s); // right
  ctx.fillRect(bx + 1 * s, by + 1 * s, 3 * s, 2 * s); // top-left
  ctx.fillRect(bx + 16 * s, by + 1 * s, 3 * s, 2 * s); // top-right
  ctx.fillRect(bx + 1 * s, by + 17 * s, 3 * s, 2 * s); // bottom-left
  ctx.fillRect(bx + 16 * s, by + 17 * s, 3 * s, 2 * s); // bottom-right

  // Inner spokes
  ctx.fillStyle = '#6B4018';
  ctx.fillRect(bx + 9 * s, by + 4 * s, 2 * s, 12 * s);
  ctx.fillRect(bx + 3 * s, by + 9 * s, 14 * s, 2 * s);

  // Center golden hub
  ctx.fillStyle = '#F1C40F';
  ctx.fillRect(bx + 8 * s, by + 8 * s, 4 * s, 4 * s);
  ctx.fillStyle = '#D4AC0D';
  ctx.fillRect(bx + 9 * s, by + 9 * s, 2 * s, 2 * s);
}

export function drawJollyRogerFlag(ctx: CanvasRenderingContext2D, x: number, y: number, frame: number): void {
  const bx = x * s, by = y * s;
  const wave = Math.sin(frame * 0.3) * 1 * s;

  // Flag pole
  ctx.fillStyle = '#8B5A2B';
  ctx.fillRect(bx, by, 2 * s, 36 * s);
  ctx.fillStyle = '#F1C40F';
  ctx.fillRect(bx - 1 * s, by - 1 * s, 4 * s, 3 * s);

  // Black banner
  ctx.fillStyle = '#111118';
  ctx.fillRect(bx + 2 * s, by + 2 * s + wave, 28 * s, 18 * s);
  ctx.fillStyle = '#22222E';
  ctx.fillRect(bx + 2 * s, by + 2 * s + wave, 28 * s, 1 * s);

  const fx = bx + 4 * s;
  const fy = by + 3 * s + wave;

  // Crossed bones (white/bone)
  ctx.fillStyle = '#EAEAEA';
  // Bone \
  ctx.fillRect(fx + 5 * s, fy + 4 * s, 2 * s, 2 * s);
  ctx.fillRect(fx + 7 * s, fy + 6 * s, 2 * s, 2 * s);
  ctx.fillRect(fx + 15 * s, fy + 12 * s, 2 * s, 2 * s);
  ctx.fillRect(fx + 17 * s, fy + 14 * s, 2 * s, 2 * s);
  // Bone /
  ctx.fillRect(fx + 17 * s, fy + 4 * s, 2 * s, 2 * s);
  ctx.fillRect(fx + 15 * s, fy + 6 * s, 2 * s, 2 * s);
  ctx.fillRect(fx + 7 * s, fy + 12 * s, 2 * s, 2 * s);
  ctx.fillRect(fx + 5 * s, fy + 14 * s, 2 * s, 2 * s);

  // Skull
  ctx.fillStyle = '#FFFFFF';
  ctx.fillRect(fx + 9 * s, fy + 5 * s, 6 * s, 5 * s);
  ctx.fillRect(fx + 10 * s, fy + 10 * s, 4 * s, 3 * s);

  // Eye sockets
  ctx.fillStyle = '#111118';
  ctx.fillRect(fx + 10 * s, fy + 7 * s, 1 * s, 2 * s);
  ctx.fillRect(fx + 13 * s, fy + 7 * s, 1 * s, 2 * s);

  // Teeth
  ctx.fillStyle = '#111118';
  ctx.fillRect(fx + 11 * s, fy + 11 * s, 1 * s, 2 * s);
  ctx.fillRect(fx + 12 * s, fy + 11 * s, 1 * s, 2 * s);

  // Luffy's Straw Hat atop the skull!
  ctx.fillStyle = '#F1C40F'; // Yellow brim
  ctx.fillRect(fx + 6 * s, fy + 4 * s, 12 * s, 2 * s);
  ctx.fillStyle = '#C0392B'; // Red ribbon
  ctx.fillRect(fx + 8 * s, fy + 3 * s, 8 * s, 1 * s);
  ctx.fillStyle = '#F1C40F'; // Hat crown
  ctx.fillRect(fx + 9 * s, fy + 1 * s, 6 * s, 2 * s);
}

export function drawTreasureChest(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  // Chest shadow
  ctx.fillStyle = 'rgba(0,0,0,0.3)';
  ctx.fillRect(bx + 1 * s, by + 18 * s, 22 * s, 3 * s);

  // Open chest base
  ctx.fillStyle = '#5A2A10';
  ctx.fillRect(bx + 2 * s, by + 8 * s, 20 * s, 11 * s);
  ctx.fillStyle = '#7A3A18';
  ctx.fillRect(bx + 3 * s, by + 9 * s, 18 * s, 9 * s);

  // Brass brackets
  ctx.fillStyle = '#D4AF37';
  ctx.fillRect(bx + 2 * s, by + 8 * s, 3 * s, 11 * s);
  ctx.fillRect(bx + 19 * s, by + 8 * s, 3 * s, 11 * s);
  ctx.fillRect(bx + 10 * s, by + 11 * s, 4 * s, 5 * s); // Lock plate
  ctx.fillStyle = '#222';
  ctx.fillRect(bx + 11 * s, by + 13 * s, 2 * s, 2 * s); // Keyhole

  // Open lid (angled back)
  ctx.fillStyle = '#5A2A10';
  ctx.fillRect(bx + 2 * s, by + 1 * s, 20 * s, 6 * s);
  ctx.fillStyle = '#3A1808';
  ctx.fillRect(bx + 3 * s, by + 2 * s, 18 * s, 4 * s);
  ctx.fillStyle = '#D4AF37';
  ctx.fillRect(bx + 2 * s, by + 1 * s, 3 * s, 6 * s);
  ctx.fillRect(bx + 19 * s, by + 1 * s, 3 * s, 6 * s);

  // Glistening gold coins and gems overflowing!
  ctx.fillStyle = '#F1C40F';
  ctx.fillRect(bx + 4 * s, by + 6 * s, 16 * s, 3 * s);
  ctx.fillRect(bx + 5 * s, by + 5 * s, 14 * s, 2 * s);
  ctx.fillStyle = '#F39C12';
  ctx.fillRect(bx + 6 * s, by + 7 * s, 4 * s, 2 * s);
  ctx.fillRect(bx + 14 * s, by + 7 * s, 4 * s, 2 * s);
  // Red ruby
  ctx.fillStyle = '#E74C3C';
  ctx.fillRect(bx + 9 * s, by + 5 * s, 2 * s, 2 * s);
  // Blue sapphire
  ctx.fillStyle = '#3498DB';
  ctx.fillRect(bx + 13 * s, by + 5 * s, 2 * s, 2 * s);
  // Green emerald
  ctx.fillStyle = '#2ECC71';
  ctx.fillRect(bx + 7 * s, by + 6 * s, 2 * s, 2 * s);
}

export function drawRumBarrels(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;

  // Bottom barrel
  drawSingleBarrel(ctx, bx, by + 10 * s, 'COLA');
  // Top barrel
  drawSingleBarrel(ctx, bx + 4 * s, by, 'SAKE');

  function drawSingleBarrel(c: CanvasRenderingContext2D, px: number, py: number, label: string) {
    // Barrel body
    c.fillStyle = '#6E4720';
    c.fillRect(px + 2 * s, py, 14 * s, 14 * s);
    c.fillRect(px, py + 2 * s, 18 * s, 10 * s); // Bulge
    // Iron hoops
    c.fillStyle = '#2C3E50';
    c.fillRect(px, py + 3 * s, 18 * s, 2 * s);
    c.fillRect(px, py + 9 * s, 18 * s, 2 * s);
    // Wood highlights
    c.fillStyle = '#8B5A2B';
    c.fillRect(px + 2 * s, py + 1 * s, 14 * s, 1 * s);
    // Spigot bung
    c.fillStyle = '#D4AF37';
    c.fillRect(px + 8 * s, py + 6 * s, 2 * s, 2 * s);
    // Stenciled text
    c.fillStyle = '#EAEAEA';
    c.font = `bold ${2 * s}px monospace`;
    c.fillText(label, px + 4 * s, py + 8 * s);
  }
}

export function drawShipCannon(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;

  // Wooden gun carriage
  ctx.fillStyle = '#5A2A10';
  ctx.fillRect(bx + 2 * s, by + 6 * s, 16 * s, 8 * s);
  // 4 Truck wheels
  ctx.fillStyle = '#2C1D11';
  ctx.fillRect(bx + 1 * s, by + 12 * s, 4 * s, 4 * s);
  ctx.fillRect(bx + 13 * s, by + 12 * s, 4 * s, 4 * s);

  // Cast iron cannon barrel (pointing out left)
  ctx.fillStyle = '#22222E';
  ctx.fillRect(bx - 3 * s, by + 2 * s, 20 * s, 5 * s);
  // Cannon muzzle ring
  ctx.fillStyle = '#333344';
  ctx.fillRect(bx - 4 * s, by + 1 * s, 2 * s, 7 * s);
  // Cannon breech knob
  ctx.fillStyle = '#1A1A24';
  ctx.fillRect(bx + 16 * s, by + 3 * s, 3 * s, 3 * s);
  // Brass reinforcement bands
  ctx.fillStyle = '#D4AF37';
  ctx.fillRect(bx + 2 * s, by + 2 * s, 2 * s, 5 * s);
  ctx.fillRect(bx + 9 * s, by + 2 * s, 2 * s, 5 * s);
}

export function drawSunnyMast(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  // Mast timber
  ctx.fillStyle = '#7A4518';
  ctx.fillRect(bx + 8 * s, by, 8 * s, 40 * s);
  ctx.fillStyle = '#5A3010';
  ctx.fillRect(bx + 8 * s, by, 2 * s, 40 * s);
  // Cross yardarm
  ctx.fillStyle = '#8B5020';
  ctx.fillRect(bx - 8 * s, by + 12 * s, 40 * s, 4 * s);
  // Rigging ropes
  ctx.strokeStyle = '#D4AF37';
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(bx + 12 * s, by);
  ctx.lineTo(bx - 8 * s, by + 40 * s);
  ctx.moveTo(bx + 12 * s, by);
  ctx.lineTo(bx + 32 * s, by + 40 * s);
  ctx.stroke();
}

export function drawMikanTrees(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  // Wooden planter box
  ctx.fillStyle = '#6E4720';
  ctx.fillRect(bx + 2 * s, by + 14 * s, 16 * s, 10 * s);
  ctx.fillStyle = '#4A2E12';
  ctx.fillRect(bx + 1 * s, by + 14 * s, 18 * s, 2 * s);
  ctx.fillStyle = '#3A2008';
  ctx.fillRect(bx + 2 * s, by + 15 * s, 16 * s, 1 * s);

  // Tree trunk
  ctx.fillStyle = '#5A3815';
  ctx.fillRect(bx + 8 * s, by + 8 * s, 4 * s, 7 * s);

  // Lush green foliage
  ctx.fillStyle = '#1E7E34';
  ctx.fillRect(bx + 2 * s, by + 2 * s, 16 * s, 9 * s);
  ctx.fillRect(bx + 4 * s, by - 1 * s, 12 * s, 4 * s);
  ctx.fillRect(bx + 1 * s, by + 4 * s, 18 * s, 5 * s);
  ctx.fillStyle = '#28A745';
  ctx.fillRect(bx + 5 * s, by + 1 * s, 10 * s, 3 * s);

  // Juicy bright orange Mikan fruits!
  ctx.fillStyle = '#FF7700';
  ctx.fillRect(bx + 4 * s, by + 3 * s, 3 * s, 3 * s);
  ctx.fillRect(bx + 12 * s, by + 4 * s, 3 * s, 3 * s);
  ctx.fillRect(bx + 8 * s, by + 7 * s, 3 * s, 3 * s);
  ctx.fillRect(bx + 14 * s, by + 8 * s, 2 * s, 2 * s);
  // Fruit shine
  ctx.fillStyle = '#FFAA33';
  ctx.fillRect(bx + 4 * s, by + 3 * s, 1 * s, 1 * s);
  ctx.fillRect(bx + 12 * s, by + 4 * s, 1 * s, 1 * s);
}

export function drawAnchorAndRope(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  const bx = x * s, by = y * s;
  // Manila hemp rope coil
  ctx.fillStyle = '#C8A06A';
  ctx.fillRect(bx + 1 * s, by + 12 * s, 18 * s, 8 * s);
  ctx.fillStyle = '#A8804A';
  ctx.fillRect(bx + 3 * s, by + 14 * s, 14 * s, 4 * s);

  // Iron anchor
  ctx.fillStyle = '#2C3E50';
  // Central shank
  ctx.fillRect(bx + 8 * s, by, 4 * s, 18 * s);
  // Cross stock
  ctx.fillRect(bx + 3 * s, by + 3 * s, 14 * s, 2 * s);
  // Top ring
  ctx.fillRect(bx + 7 * s, by - 2 * s, 6 * s, 2 * s);
  // Curved flukes / arms
  ctx.fillRect(bx + 2 * s, by + 14 * s, 3 * s, 4 * s);
  ctx.fillRect(bx + 15 * s, by + 14 * s, 3 * s, 4 * s);
  ctx.fillRect(bx + 4 * s, by + 16 * s, 12 * s, 3 * s);
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
