"use client";

import { useState } from "react";

import { useZou } from "@/lib/store";

/**
 * The planning input: type a feature request, submit to `POST /api/plan`.
 *
 * The daemon plans with its built-in planner, or with `ZOU_PLANNER_CMD` (any
 * LLM CLI that prints a JSON plan) when the launch environment sets it — the
 * UI is identical either way.
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

  const action =
    "h-11 shrink-0 justify-center border-4 border-ink px-5 text-sm font-black uppercase tracking-[0.06em] shadow-brutal-sm hover:-translate-y-0.5 hover:shadow-brutal active:translate-y-0 active:shadow-brutal-sm disabled:translate-y-0 disabled:bg-ground disabled:border-ink/40 disabled:text-ink/40 disabled:shadow-none";

  return (
    <form
      onSubmit={submit}
      className="flex w-full flex-col gap-2 sm:flex-row sm:items-center"
    >
      <label htmlFor="plan-prompt" className="sr-only">
        Feature request
      </label>
      <input
        id="plan-prompt"
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        placeholder="Describe a feature to decompose into a task DAG…"
        disabled={busy}
        className="h-11 min-w-0 flex-1 border-4 border-ink bg-contrast px-3 text-base font-bold text-ink shadow-brutal-sm placeholder:text-ink/40 disabled:opacity-50"
      />
      <button
        type="submit"
        disabled={busy || !prompt.trim()}
        aria-label={busy ? "Planning…" : "Plan"}
        className={`${action} bg-emphasis text-ink`}
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
        className={`${action} bg-signal text-ink`}
      >
        Run{todoCount > 0 ? ` (${todoCount})` : ""}
      </button>
    </form>
  );
}
