"use client";

import { useState } from "react";

import { useZou } from "@/lib/store";

/**
 * The planning input: type a feature request, submit to `POST /api/plan`.
 *
 * The daemon's planner is a Phase-2 mock (a fixed 3-task chain) today; this
 * box is the seam a real LLM planner drops into without any UI change.
 */
export default function PlanBar() {
  const [prompt, setPrompt] = useState("");
  const plan = useZou((s) => s.plan);
  const runAll = useZou((s) => s.runAll);
  const busy = useZou((s) => s.busy);
  const todoCount = useZou(
    (s) => s.tasks.filter((t) => t.status === "TODO").length,
  );

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const text = prompt.trim();
    if (!text || busy) return;
    await plan(text);
    setPrompt("");
  };

  return (
    <form onSubmit={submit} className="flex items-center gap-2 w-full">
      <input
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        placeholder="Describe a feature to decompose into a task DAG…"
        disabled={busy}
        className="flex-1 min-w-0 rounded-md border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm text-zinc-100 placeholder:text-zinc-600 focus:outline-none focus:ring-2 focus:ring-emerald-600/60 focus:border-emerald-600 disabled:opacity-50"
      />
      <button
        type="submit"
        disabled={busy || !prompt.trim()}
        className="shrink-0 rounded-md bg-emerald-700 hover:bg-emerald-600 disabled:opacity-40 disabled:hover:bg-emerald-700 px-4 py-2 text-sm font-medium text-white transition-colors"
      >
        {busy ? "…" : "Plan"}
      </button>
      <button
        type="button"
        onClick={() => void runAll()}
        disabled={busy || todoCount === 0}
        title={
          todoCount === 0
            ? "No TODO tasks are ready to run"
            : `Run ${todoCount} ready task${todoCount === 1 ? "" : "s"}`
        }
        className="shrink-0 rounded-md border border-zinc-700 bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 px-4 py-2 text-sm font-medium text-zinc-200 transition-colors"
      >
        Run{todoCount > 0 ? ` (${todoCount})` : ""}
      </button>
    </form>
  );
}
