"use client";

import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { useZou } from "@/lib/store";
import { isActive } from "@/lib/status";
import type { Task } from "@/lib/types";

export interface CrewProfile {
  id: string;
  name: string;
  role: string;
  specialty: string;
  emoji: string;
  bounty: string;
  quote: string;
  description: string;
}

export const CREW_DOSSIER: Record<string, CrewProfile> = {
  luffy: {
    id: "luffy",
    name: "Monkey D. Luffy",
    role: "Captain & Strategic Lead",
    specialty: "High-level orchestration, feature architecture, bold initiatives",
    emoji: "👒",
    bounty: "3,000,000,000 ฿",
    quote: "I'm gonna be King of the Pirates! Let's build something epic!",
    description: "Leads the Straw Hat Pirates. Breaks down complex epics and guides overarching architectural decisions with unshakable optimism.",
  },
  zoro: {
    id: "zoro",
    name: "Roronoa Zoro",
    role: "Combatant & Core Logic",
    specialty: "Deep backend refactoring, data structures, performance slicing",
    emoji: "⚔️",
    bounty: "1,111,000,000 ฿",
    quote: "Nothing happened. The bugs have been cut down.",
    description: "Master of Three-Sword Style. Slices through stubborn bugs, concurrency deadlocks, and messy legacy code with zero wasted motion.",
  },
  nami: {
    id: "nami",
    name: "Nami",
    role: "Navigator & Chief Planner",
    specialty: "DAG task routing, dependency mapping, scope estimation",
    emoji: "🗺️",
    bounty: "366,000,000 ฿",
    quote: "The weather in this codebase looks stormy, but I have mapped our route!",
    description: "Expert navigator and cartographer. Dissects feature requests into safe, acyclic task graphs and directs the crew to port.",
  },
  usopp: {
    id: "usopp",
    name: "Usopp",
    role: "Sniper & Test Engineer",
    specialty: "Precision bugfixes, edge case testing, tool crafting",
    emoji: "🎯",
    bounty: "500,000,000 ฿",
    quote: "I have 8,000 unit tests behind me! Sniped that regression from 2 miles away.",
    description: "Sharp-eyed sniper and inventor. Crafts tests, catches regression edge cases, and delivers surgical fixes with pinpoint precision.",
  },
  sanji: {
    id: "sanji",
    name: "Sanji",
    role: "Cook & UI/UX Specialist",
    specialty: "Frontend polish, responsive styling, documentation & delightful UX",
    emoji: "🍳",
    bounty: "1,032,000,000 ฿",
    quote: "Only the finest, well-crafted interfaces and documentation for our users!",
    description: "Black Leg master chef. Refuses to let sloppy UI or stale documentation leave the kitchen, serving only five-star presentation.",
  },
  chopper: {
    id: "chopper",
    name: "Tony Tony Chopper",
    role: "Doctor & Crash Diagnostics",
    specialty: "Error healing, panic diagnosis, bug triage & test repairs",
    emoji: "🩺",
    bounty: "1,000 ฿",
    quote: "Calling me a genius doctor won't make me happy, you bastard! 💕 I will heal this crash!",
    description: "Doctor and master of Rumble Balls. Diagnoses stack traces, recovers crashed states, and heals panics with surgical accuracy.",
  },
  robin: {
    id: "robin",
    name: "Nico Robin",
    role: "Archaeologist & Architecture Historian",
    specialty: "Codebase context synthesis, ADRs, documentation & git archaeology",
    emoji: "📜",
    bounty: "930,000,000 ฿",
    quote: "History holds the secrets of why this was built. I shall uncover the truth.",
    description: "Archaeologist of Ohara. Reads ancient commits, deciphers complex systems architecture, and writes pristine living documentation.",
  },
  franky: {
    id: "franky",
    name: "Franky (Cutty Flam)",
    role: "Shipwright & Dev Tools Engineer",
    specialty: "Infrastructure, build pipelines, Docker containers, toolchain & CI/CD",
    emoji: "⚙️",
    bounty: "394,000,000 ฿",
    quote: "SUUUPER! This build pipeline is fueled by pure cola and unstoppable iron!",
    description: "Cyborg shipwright of the Thousand Sunny. Forges blazing fast build workflows, hardens containers, and optimizes project infrastructure.",
  },
  brook: {
    id: "brook",
    name: "Brook (Soul King)",
    role: "Musician & Performance Virtuoso",
    specialty: "Latency profiling, memory benchmarks, allocations & telemetry cadence",
    emoji: "🎻",
    bounty: "383,000,000 ฿",
    quote: "Yo-ho-ho-ho! This hot-path gives me chills down to the bone — even though I have no skin!",
    description: "Soul King virtuoso. Listens to execution harmonics, shaves milliseconds off latency, and tunes memory allocation cadence to perfection.",
  },
  jinbe: {
    id: "jinbe",
    name: "Jinbe (Knight of the Sea)",
    role: "Helmsman & Release Navigator",
    specialty: "Git rebasing, merge currents, conflict resolution & stable release steering",
    emoji: "🌊",
    bounty: "1,100,000,000 ฿",
    quote: "Steer through the roughest merge waves with unwavering resolve. I hold the helm.",
    description: "First Son of the Sea. Navigates treacherous git merge conflicts, steers turbulent rebases, and docks flawless stable releases.",
  },
};

interface CrewModalProps {
  memberId: string;
  onClose: () => void;
}

export default function CrewModal({ memberId, onClose }: CrewModalProps) {
  const tasks = useZou((s) => s.tasks);
  const select = useZou((s) => s.select);
  const profile = CREW_DOSSIER[memberId] ?? {
    id: memberId,
    name: memberId.toUpperCase(),
    role: "Crew Member",
    specialty: "Specialist agent",
    emoji: "🏴‍☠️",
    bounty: "0 ฿",
    quote: "Ready for orders!",
    description: "Straw Hat crew member.",
  };

  const [provider, setProvider] = useState("opencode");
  const [model, setModel] = useState("");
  const [isCustom, setIsCustom] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void api.crew().then((res) => {
      if (!active) return;
      const entry = res.crew.find((c) => c.id === memberId);
      if (entry) {
        setProvider(entry.provider);
        setModel(entry.model ?? "");
        setIsCustom(entry.is_custom);
      }
      setLoading(false);
    }).catch(() => {
      if (active) setLoading(false);
    });
    return () => {
      active = false;
    };
  }, [memberId]);

  const assignedTasks = tasks.filter((t) => {
    if (t.crew_member) return t.crew_member.toLowerCase().includes(memberId);
    return false;
  });

  const handleSave = async () => {
    setSaving(true);
    setMsg(null);
    try {
      const updated = await api.updateCrew(memberId, {
        provider,
        model: model.trim() || undefined,
      });
      setIsCustom(updated.is_custom);
      setMsg("Saved! Model assigned to " + profile.name);
    } catch (e: unknown) {
      setMsg("Error: " + (e instanceof Error ? e.message : String(e)));
    } finally {
      setSaving(false);
    }
  };

  const handleReset = async () => {
    setSaving(true);
    setMsg(null);
    try {
      const updated = await api.updateCrew(memberId, { reset: true });
      setIsCustom(false);
      setModel("");
      setProvider(updated.provider);
      setMsg("Reset to default agent model");
    } catch (e: unknown) {
      setMsg("Error: " + (e instanceof Error ? e.message : String(e)));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="crew-modal-title"
      className="fixed inset-0 z-50 flex items-center justify-center bg-ink/75 p-3 backdrop-blur-xs"
      onClick={onClose}
    >
      <div
        className="flex max-h-[90vh] w-full max-w-xl flex-col border-4 border-ink bg-contrast shadow-brutal"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-start justify-between border-b-4 border-ink bg-signal p-4">
          <div className="min-w-0 flex-1">
            <div className="text-[10px] font-black uppercase tracking-[0.14em] text-ink/80">
              STRAW HAT PIRATES · BOUNTY {profile.bounty}
            </div>
            <h2 id="crew-modal-title" className="flex items-center gap-2 text-2xl font-black uppercase text-ink">
              <span>{profile.emoji}</span>
              <span>{profile.name}</span>
            </h2>
            <div className="mt-1 flex flex-wrap items-center gap-1.5">
              <span className="border border-ink bg-ink px-1.5 py-0.5 text-[10px] font-black uppercase tracking-wider text-emphasis">
                {profile.role}
              </span>
              {isCustom ? (
                <span className="border border-ink bg-emphasis px-1.5 py-0.5 text-[10px] font-black uppercase text-ink">
                  ⚡ Custom Model Active
                </span>
              ) : (
                <span className="border border-ink bg-ground px-1.5 py-0.5 text-[10px] font-bold uppercase text-ink/70">
                  Default Model
                </span>
              )}
            </div>
          </div>
          <button
            onClick={onClose}
            aria-label="Close dossier"
            className="grid size-8 place-items-center border-2 border-ink bg-contrast font-black text-ink hover:bg-emphasis focus-visible:outline-4"
          >
            ✕
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          <blockquote className="border-l-4 border-ink bg-ground p-3 text-sm font-semibold italic text-ink/90">
            “{profile.quote}”
          </blockquote>

          <div className="text-xs leading-relaxed text-ink/80">
            {profile.description}
          </div>

          {/* Model & Provider Switcher */}
          <div className="border-2 border-ink bg-ground p-3 shadow-brutal-sm">
            <div className="mb-2 flex items-center justify-between">
              <span className="text-[11px] font-black uppercase tracking-wider">
                Assigned Agent Model
              </span>
              {loading && <span className="text-[10px] font-bold text-ink/50">loading…</span>}
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 mb-3">
              <div>
                <label className="block text-[10px] font-black uppercase text-ink/70 mb-1">
                  Provider
                </label>
                <select
                  value={provider}
                  onChange={(e) => setProvider(e.target.value)}
                  disabled={saving}
                  className="w-full border-2 border-ink bg-contrast px-2 py-1 text-xs font-bold shadow-brutal-sm"
                >
                  <option value="opencode">OpenCode (opencode)</option>
                  <option value="antigravity">Google Antigravity (agy)</option>
                </select>
              </div>

              <div>
                <label className="block text-[10px] font-black uppercase text-ink/70 mb-1">
                  Model ID
                </label>
                <input
                  list="crew-model-presets"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  disabled={saving}
                  placeholder="e.g. anthropic/claude-3.7-sonnet"
                  className="w-full border-2 border-ink bg-contrast px-2 py-1 text-xs font-mono font-bold shadow-brutal-sm"
                />
                <datalist id="crew-model-presets">
                  <option value="anthropic/claude-3.7-sonnet" />
                  <option value="anthropic/claude-3.5-sonnet" />
                  <option value="openai/gpt-4o" />
                  <option value="google/gemini-2.5-pro" />
                  <option value="meta-llama/llama-3.3-70b-instruct" />
                </datalist>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2">
              <button
                onClick={() => void handleSave()}
                disabled={saving}
                className="border-2 border-ink bg-emphasis px-3 py-1 text-xs font-black uppercase tracking-wider shadow-brutal-sm hover:bg-contrast disabled:opacity-50"
              >
                {saving ? "Saving…" : "Save Model"}
              </button>
              {isCustom && (
                <button
                  onClick={() => void handleReset()}
                  disabled={saving}
                  className="border-2 border-ink bg-contrast px-3 py-1 text-xs font-black uppercase tracking-wider shadow-brutal-sm hover:bg-signal hover:text-contrast disabled:opacity-50"
                >
                  Reset to Default
                </button>
              )}
              {msg && <span className="text-[11px] font-bold text-ink">{msg}</span>}
            </div>
          </div>

          {/* Assigned Tasks */}
          <div>
            <div className="mb-2 flex items-center justify-between border-b-2 border-ink pb-1">
              <span className="text-[11px] font-black uppercase tracking-wider">
                Assigned Tasks ({assignedTasks.length})
              </span>
            </div>
            {assignedTasks.length === 0 ? (
              <p className="text-xs text-ink/50 italic">
                No active tasks assigned to {profile.name} yet.
              </p>
            ) : (
              <div className="space-y-1.5 max-h-40 overflow-y-auto">
                {assignedTasks.map((t) => (
                  <div
                    key={t.id}
                    onClick={() => {
                      select(t.id);
                      onClose();
                    }}
                    className="flex cursor-pointer items-center justify-between border-2 border-ink bg-contrast px-2 py-1 text-xs font-bold shadow-brutal-sm hover:bg-emphasis"
                  >
                    <span className="truncate pr-2">{t.title}</span>
                    <span className="shrink-0 border border-ink bg-ground px-1 text-[9px] uppercase">
                      {t.status.replaceAll("_", " ")}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Footer */}
        <div className="flex justify-end border-t-4 border-ink bg-ground p-3">
          <button
            onClick={onClose}
            className="border-2 border-ink bg-contrast px-4 py-1 text-xs font-black uppercase tracking-wider shadow-brutal-sm hover:bg-emphasis"
          >
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
