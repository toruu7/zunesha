"use client";

import { useEffect, useState } from "react";

import { useZou } from "@/lib/store";
import type { Task } from "@/lib/types";
import LogViewer from "./log-viewer";

/** Color a unified diff line: additions green, deletions red, headers dim. */
function diffLineClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---")) return "text-zinc-500";
  if (line.startsWith("diff ") || line.startsWith("index "))
    return "text-zinc-500 font-semibold";
  if (line.startsWith("@@")) return "text-sky-400";
  if (line.startsWith("+")) return "text-emerald-400";
  if (line.startsWith("-")) return "text-red-400";
  return "text-zinc-300";
}

function DiffView({ diff }: { diff: string }) {
  if (!diff.trim()) {
    return (
      <p className="text-sm text-zinc-600 italic p-3">
        No changes captured for this task.
      </p>
    );
  }
  return (
    <pre className="p-3 font-mono text-[12px] leading-relaxed overflow-auto max-h-72 bg-black/60">
      {diff.split("\n").map((line, i) => (
        <div key={i} className={`whitespace-pre-wrap break-all ${diffLineClass(line)}`}>
          {line || " "}
        </div>
      ))}
    </pre>
  );
}

/**
 * The human-in-the-loop review surface.
 *
 * Opens for any task; shows description, live output, and the captured diff.
 * Approve/Reject only apply to REVIEW tasks (the API enforces that too — a
 * non-REVIEW task yields 409, surfaced through the store's error banner).
 */
export default function ReviewModal({ task }: { task: Task }) {
  const select = useZou((s) => s.select);
  const approve = useZou((s) => s.approve);
  const reject = useZou((s) => s.reject);
  const runOne = useZou((s) => s.runOne);
  const recover = useZou((s) => s.recover);
  const busy = useZou((s) => s.busy);
  const error = useZou((s) => s.error);
  const clearError = useZou((s) => s.clearError);
  const [feedback, setFeedback] = useState("");
  const [tab, setTab] = useState<"diff" | "logs">("diff");

  // Close on Escape.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") select(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [select]);

  const isReview = task.status === "REVIEW";
  const isTodo = task.status === "TODO";
  // A live worker owns these (recover releases either).
  const isWorkerOwned =
    task.status === "IN_PROGRESS" ||
    task.status === "EVALUATING" ||
    task.status === "RESOLVING_CONFLICTS";
  // Parked after 3 failed validations; the human frees it back to the queue.
  const isFailed = task.status === "FAILED_NEEDS_HUMAN";
  const close = () => select(null);

  return (
    // Backdrop
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
      onClick={close}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={`Review task ${task.id}`}
        className="w-full max-w-3xl max-h-[85vh] flex flex-col rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <header className="flex items-start gap-3 px-4 py-3 border-b border-zinc-800">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <code className="text-xs text-zinc-500 font-mono">{task.id}</code>
              <span className="text-[10px] font-semibold tracking-wide px-1.5 py-0.5 rounded bg-zinc-800 text-zinc-300">
                {task.status.replaceAll("_", " ")}
              </span>
              {task.retry_count > 0 && (
                <span
                  title="Failed validations in the current review cycle (max 3)"
                  className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-amber-950 text-amber-400 border border-amber-900"
                >
                  retries {task.retry_count}/3
                </span>
              )}
            </div>
            <h2 className="text-base font-semibold text-zinc-100 mt-1 break-words">
              {task.title}
            </h2>
          </div>
          <button
            onClick={close}
            aria-label="Close"
            className="shrink-0 rounded p-1 text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path
                d="M4 4l8 8M12 4l-8 8"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
              />
            </svg>
          </button>
        </header>

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {/* Phase 9: why this task left REVIEW without the human's diff
              landing — shown while the auto-merge agent runs. */}
          {task.status === "RESOLVING_CONFLICTS" && (
            <section className="rounded-md border border-orange-800 bg-orange-950/50 px-3 py-2 text-xs leading-relaxed text-orange-200">
              <strong className="font-semibold">Merge conflict detected.</strong>{" "}
              The repository changed while this task was running, so the
              approved diff no longer applied. Its workspace has been rebased
              onto the current code with conflict markers and an agent is
              reconciling them now — the result returns here for a second,
              clean approval. Nothing was written to your working tree.
            </section>
          )}

          {task.description.trim() && (
            <section>
              <h3 className="text-[11px] font-mono uppercase tracking-wide text-zinc-500 mb-1.5">
                Description
              </h3>
              <p className="text-sm text-zinc-300 whitespace-pre-wrap leading-relaxed">
                {task.description}
              </p>
            </section>
          )}

          {/* Diff / logs tabs */}
          <section>
            <div className="flex gap-1 mb-1.5">
              {(["diff", "logs"] as const).map((t) => (
                <button
                  key={t}
                  onClick={() => setTab(t)}
                  className={`px-2.5 py-1 text-[11px] font-mono uppercase tracking-wide rounded ${
                    tab === t
                      ? "bg-zinc-800 text-zinc-100"
                      : "text-zinc-500 hover:text-zinc-300"
                  }`}
                >
                  {t}
                </button>
              ))}
            </div>
            {tab === "diff" ? (
              <DiffView diff={task.diff} />
            ) : (
              <LogViewer taskId={task.id} />
            )}
          </section>

          {/* Feedback + actions for REVIEW and parked (FAILED_NEEDS_HUMAN)
              tasks — both feed the next attempt via the reject endpoint */}
          {(isReview || isFailed) && (
            <section className="space-y-2">
              <label
                htmlFor="feedback"
                className="block text-[11px] font-mono uppercase tracking-wide text-zinc-500"
              >
                {isFailed
                  ? "What should happen next? (optional — feeds the next attempt)"
                  : "Reviewer feedback (optional — feeds the next attempt)"}
              </label>
              <textarea
                id="feedback"
                value={feedback}
                onChange={(e) => setFeedback(e.target.value)}
                rows={3}
                placeholder={
                  isFailed
                    ? "Validation failed 3 times. Why, and what should the agent do differently?"
                    : "Why is this rejected? What should the agent do differently?"
                }
                className="w-full rounded-md border border-zinc-700 bg-zinc-950 px-3 py-2 text-sm text-zinc-100 placeholder:text-zinc-600 focus:outline-none focus:ring-2 focus:ring-violet-600/50 resize-y"
              />
            </section>
          )}
        </div>

        {/* Failures surface here as well as in the page banner: this dialog
            is a full-screen overlay, so the banner alone would be hidden at
            exactly the moment it matters (a merge conflict on Approve). */}
        {error && (
          <div className="px-4 pb-3">
            <div
              role="alert"
              className="rounded-md border border-red-800 bg-red-950/60 px-3 py-2 text-xs leading-relaxed text-red-200 break-words"
            >
              {error}
              <button
                onClick={clearError}
                className="ml-2 font-medium text-red-400 hover:text-red-200"
              >
                dismiss
              </button>
            </div>
          </div>
        )}

        {/* Footer actions */}
        <footer className="flex items-center gap-2 px-4 py-3 border-t border-zinc-800">
          {isReview ? (
            <>
              <button
                onClick={() => void approve(task.id)}
                disabled={busy}
                className="rounded-md bg-emerald-700 hover:bg-emerald-600 disabled:opacity-40 px-4 py-2 text-sm font-medium text-white transition-colors"
              >
                Approve
              </button>
              <button
                onClick={() => void reject(task.id, feedback)}
                disabled={busy}
                className="rounded-md bg-red-800 hover:bg-red-700 disabled:opacity-40 px-4 py-2 text-sm font-medium text-white transition-colors"
              >
                Reject
              </button>
            </>
          ) : isTodo ? (
            <button
              onClick={() => void runOne(task.id)}
              disabled={busy}
              className="rounded-md bg-amber-700 hover:bg-amber-600 disabled:opacity-40 px-4 py-2 text-sm font-medium text-white transition-colors"
            >
              Run agent
            </button>
          ) : isFailed ? (
            <button
              onClick={() => void reject(task.id, feedback)}
              disabled={busy}
              title="Send the parked task back to the queue with a fresh retry budget (workspace kept)."
              className="rounded-md bg-amber-700 hover:bg-amber-600 disabled:opacity-40 px-4 py-2 text-sm font-medium text-white transition-colors"
            >
              Send back to queue
            </button>
          ) : isWorkerOwned ? (
            <button
              onClick={() => void recover(task.id)}
              disabled={busy}
              title="Worker died or was interrupted? Free its claim so the task can run again (workspace kept)."
              className="rounded-md bg-zinc-700 hover:bg-zinc-600 disabled:opacity-40 px-4 py-2 text-sm font-medium text-zinc-100 transition-colors"
            >
              Release stuck task
            </button>
          ) : null}
          <button
            onClick={close}
            className="ml-auto rounded-md border border-zinc-700 px-4 py-2 text-sm font-medium text-zinc-300 hover:bg-zinc-800 transition-colors"
          >
            Close
          </button>
        </footer>
      </div>
    </div>
  );
}
