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

// Dedicated Thousand Sunny Deck Stations for the 5 Straw Hat Pirates
export const SUNNY_STATIONS: Record<string, { deskX: number; deskY: number; label: string }> = {
  luffy: { deskX: 168, deskY: 48, label: 'LUFFY' },  // Center helm / captain bridge
  zoro:  { deskX: 60,  deskY: 65, label: 'ZORO' },   // Port side swords & training
  nami:  { deskX: 268, deskY: 65, label: 'NAMI' },   // Starboard cartography & mikans
  usopp: { deskX: 80,  deskY: 155, label: 'USOPP' }, // Port workshop & sniping bench
  sanji: { deskX: 248, deskY: 155, label: 'SANJI' }, // Starboard galley bar & kitchen
};

export function computeLayout(
  agents: Array<{ id: string; name: string; crew_member?: string }>,
): AgentLayout[] {
  const n = agents.length;
  if (n === 0) return [];

  return agents.map((a, i) => {
    const raw = (a.crew_member ?? a.id).toLowerCase();
    const stationKey = ['luffy', 'zoro', 'nami', 'usopp', 'sanji'].find((k) => raw.includes(k));
    const station = stationKey ? SUNNY_STATIONS[stationKey] : null;

    let deskX: number;
    let deskY: number;

    if (station) {
      deskX = station.deskX;
      deskY = station.deskY;
    } else {
      // Fallback grid
      const slotW = 55;
      const slotH = 70;
      const cols = 5;
      const col = i % cols;
      const row = Math.floor(i / cols);
      deskX = 20 + col * slotW;
      deskY = 50 + row * slotH;
    }

    const lookupKey = a.crew_member ?? a.id;
    const { color, secondary: secondaryColor } = agentColor(lookupKey);

    return {
      id: a.id,
      index: i,
      deskX,
      deskY,
      color,
      secondaryColor,
      label: station?.label ?? (a.crew_member ? a.crew_member.toUpperCase() : a.name.toUpperCase().substring(0, 10)),
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
