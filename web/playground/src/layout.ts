// Dynamic layout engine — computes desk positions for any number of agents

export interface AgentLayout {
  id: string;
  index: number;
  deskX: number;
  deskY: number;
  color: string;
  secondaryColor: string;
  label: string;
  name: string;
  emoji: string;
  crew_member?: string;
}

// Color pool — first 4 match the known agents, rest for extras
const COLOR_POOL: Array<{ color: string; secondary: string }> = [
  { color: '#4A90D9', secondary: '#2C5F8A' }, // blue   — main/Jarvis
  { color: '#2ECC71', secondary: '#1A8B4C' }, // green  — analista
  { color: '#E67E22', secondary: '#A85C18' }, // orange — rafael/Felipe
  { color: '#9B59B6', secondary: '#6C3483' }, // purple — bibliotecario
  { color: '#E74C3C', secondary: '#A93226' }, // red
  { color: '#1ABC9C', secondary: '#148F77' }, // teal
  { color: '#F39C12', secondary: '#B7770D' }, // amber
  { color: '#3498DB', secondary: '#2171B5' }, // sky blue
  { color: '#95A5A6', secondary: '#566A71' }, // slate
  { color: '#D35400', secondary: '#9A3E00' }, // burnt orange
  { color: '#27AE60', secondary: '#1A7A43' }, // emerald
  { color: '#8E44AD', secondary: '#6A2F82' }, // violet
  { color: '#C0392B', secondary: '#8E2215' }, // crimson
  { color: '#16A085', secondary: '#0E6B5A' }, // seafoam
  { color: '#7F8C8D', secondary: '#566566' }, // cool grey
  { color: '#D98C4A', secondary: '#A86030' }, // warm gold
];

// Stable color by agent ID — reserved IDs first, then deterministic hash
const RESERVED: Record<string, number> = {
  main: 0, analista: 1, rafael: 2, bibliotecario: 3,
};

const CREW_COLORS: Record<string, { color: string; secondary: string }> = {
  luffy: { color: '#E74C3C', secondary: '#24488A' }, // Red vest, denim shorts
  zoro: { color: '#27AE60', secondary: '#143820' },  // Moss green, dark coat
  nami: { color: '#E67E22', secondary: '#2980B9' },  // Orange hair, ocean blue
  usopp: { color: '#8B5A2B', secondary: '#556B2F' }, // Brown satchel, olive bandana
  sanji: { color: '#2C3E50', secondary: '#F1C40F' }, // Midnight suit, gold hair
};

export function agentColor(id: string): { color: string; secondary: string } {
  const lower = id.toLowerCase();
  for (const [key, val] of Object.entries(CREW_COLORS)) {
    if (lower.includes(key)) return val;
  }
  if (id in RESERVED) return COLOR_POOL[RESERVED[id]];
  // Deterministic hash of id string
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return COLOR_POOL[4 + (h % (COLOR_POOL.length - 4))];
}

// Canvas usable zone (logical px): x 10-350, y 24-255 (above status bar)
const ZONE_X0 = 10;
const ZONE_Y0 = 24;
const ZONE_W  = 340;
const ZONE_H  = 212;

// Space each desk slot occupies (logical px)
const SLOT_W = 50;
const SLOT_H = 68;

export function computeLayout(
  agents: Array<{ id: string; name: string; crew_member?: string }>,
): AgentLayout[] {
  const n = agents.length;
  if (n === 0) return [];

  // Grid dimensions
  const maxCols = Math.floor(ZONE_W / SLOT_W);
  const maxRows = Math.floor(ZONE_H / SLOT_H);

  let cols = Math.min(Math.ceil(Math.sqrt(n * SLOT_W / SLOT_H)), maxCols);
  let rows = Math.ceil(n / cols);
  // If rows overflow, bump columns
  while (rows > maxRows && cols < maxCols) { cols++; rows = Math.ceil(n / cols); }

  // Center the grid
  const gridW = cols * SLOT_W;
  const gridH = rows * SLOT_H;
  const startX = ZONE_X0 + Math.floor((ZONE_W - gridW) / 2);
  const startY = ZONE_Y0 + Math.floor((ZONE_H - gridH) / 2);

  return agents.map((a, i) => {
    const col = i % cols;
    const row = Math.floor(i / cols);
    const slotCX = startX + col * SLOT_W + Math.floor(SLOT_W / 2);
    const slotTY = startY + row * SLOT_H;
    // Character drawn at (deskX, deskY); desk is centered on character
    const deskX = slotCX - 8; // half of CHAR_W=16
    const deskY = slotTY;
    const lookupKey = a.crew_member ?? a.id;
    const { color, secondary: secondaryColor } = agentColor(lookupKey);

    return {
      id: a.id,
      index: i,
      deskX,
      deskY,
      color,
      secondaryColor,
      label: a.crew_member ? a.crew_member.toUpperCase() : a.name.toUpperCase().substring(0, 10),
      name: a.name,
      emoji: agentEmoji(lookupKey),
      crew_member: a.crew_member,
    };
  });
}

export function agentEmoji(id: string): string {
  const lower = id.toLowerCase();
  if (lower.includes('luffy')) return '👒';
  if (lower.includes('zoro')) return '⚔️';
  if (lower.includes('nami')) return '🗺️';
  if (lower.includes('usopp')) return '🎯';
  if (lower.includes('sanji')) return '🍳';
  const map: Record<string, string> = {
    main: '🤖', analista: '🔍', rafael: '📱', bibliotecario: '📚',
  };
  return map[lower] ?? '🏴‍☠️';
}
