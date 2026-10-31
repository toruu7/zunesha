"use client";

import { useState } from "react";

import AttachmentList from "@/components/attachment-list";
import { useAttachments } from "@/lib/use-attachments";
import { useZou } from "@/lib/store";

/**
 * The planning input: type a feature request, submit to `POST /api/plan`.
 *
 * Files/images are read locally and sent as base64. The daemon stores them
 * under `.zou/attachments/` and appends their absolute paths to the planner
 * prompt and every resulting task, so the agent can inspect screenshots or
 * specs with its normal file tool.
 */
export default function PlanBar() {
  const [prompt, setPrompt] = useState("");
  const { attachments, errors, add, remove, clear, payload } = useAttachments();
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
    await plan(text, payload());
    if (!useZou.getState().error) {
      setPrompt("");
      clear();
    }
  };

  const action =
    "h-11 w-full shrink-0 justify-center border-4 border-ink px-5 text-sm font-black uppercase tracking-[0.06em] shadow-brutal-sm hover:-translate-y-0.5 hover:shadow-brutal active:translate-y-0 active:shadow-brutal-sm disabled:translate-y-0 disabled:bg-ground disabled:border-ink/40 disabled:text-ink/40 disabled:shadow-none sm:w-auto";

  return (
    <form
      onSubmit={submit}
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes("Files")) event.preventDefault();
      }}
      onDrop={(event) => {
        if (!event.dataTransfer.files.length) return;
        event.preventDefault();
        void add(event.dataTransfer.files);
      }}
      className="flex w-full flex-col gap-2"
    >
      <div className="flex w-full flex-col gap-2 sm:flex-row sm:items-center">
        <label htmlFor="plan-prompt" className="sr-only">
          Feature request
        </label>
        <input
          id="plan-prompt"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          onPaste={(event) => {
            if (event.clipboardData.files.length) {
              event.preventDefault();
              void add(event.clipboardData.files);
            }
          }}
          placeholder="Describe a feature, or drop a screenshot/spec…"
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
      </div>
      <AttachmentList
        attachments={attachments}
        errors={errors}
        disabled={busy}
        onAdd={add}
        onRemove={remove}
      />
    </form>
  );
}
