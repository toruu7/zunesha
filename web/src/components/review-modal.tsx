"use client";

import { useEffect, useState } from "react";

import { useZou } from "@/lib/store";
import type { Task } from "@/lib/types";
import LogViewer from "./log-viewer";

/** Colour a unified diff line within the press-sheet palette. */
function diffLineClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---")) return "text-ink/60";
  if (line.startsWith("diff ") || line.startsWith("index "))
    return "text-ink/65 font-bold";
  if (line.startsWith("@@")) return "bg-field font-bold text-ink";
  if (line.startsWith("+")) return "bg-emphasis/60 text-ink";
  if (line.startsWith("-")) return "bg-signal/15 text-signal-ink";
  return "text-ink/85";
}

function DiffView({ diff }: { diff: string }) {
  if (!diff.trim()) {
    return (
      <p className="border-[3px] border-ink bg-contrast p-3 text-sm font-medium italic text-ink/60">
        No changes captured for this task.
      </p>
    );
  }
  return (
    <pre className="border-[3px] border-ink bg-contrast p-3 font-mono text-[12px] leading-relaxed overflow-auto max-h-72">
      {diff.split("\n").map((line, i) => (
        <div
          key={i}
          className={`whitespace-pre-wrap break-all px-1 ${diffLineClass(line)}`}
        >
          {line || " "}
        </div>
      ))}
    </pre>
  );
}

// Shared heavy control: square, inked, with a hard shadow that lifts on hover
// and settles back when pressed. Feedback is immediate (no transition).
const ACTION =
  "border-4 border-ink px-4 py-2 text-sm font-black uppercase tracking-[0.06em] text-ink shadow-brutal-sm hover:-translate-y-0.5 hover:shadow-brutal active:translate-y-0 active:shadow-brutal-sm disabled:translate-y-0 disabled:bg-ground disabled:border-ink/40 disabled:text-ink/40 disabled:shadow-none";

const LABEL =
  "block border-b-[3px] border-ink pb-1 text-[11px] font-black uppercase tracking-[0.1em]";

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
      className="fixed inset-0 z-50 flex items-center justify-center overflow-y-auto bg-ink/75 p-2 sm:p-6"
      onClick={close}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={`Review task ${task.id}`}
        className="flex w-full max-w-3xl max-h-[calc(100dvh-1rem)] flex-col border-4 border-ink bg-ground shadow-[10px_10px_0_0_#000000] sm:max-h-[88vh]"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header: the task's own signal strip. */}
        <header className="flex items-start gap-3 border-b-4 border-ink bg-signal px-3 py-3 sm:px-4">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <code className="border-2 border-ink bg-ink px-1.5 py-0.5 font-mono text-[11px] font-bold text-emphasis">
                {task.id}
              </code>
              <span className="border-2 border-ink bg-contrast px-1.5 py-0.5 text-[10px] font-black uppercase tracking-[0.06em]">
                {task.status.replaceAll("_", " ")}
              </span>
              {task.retry_count > 0 && (
                <span
                  title="Failed validations in the current review cycle (max 3)"
                  className="border-2 border-ink bg-contrast px-1.5 py-0.5 font-mono text-[10px] font-bold text-signal-ink"
                >
                  retries {task.retry_count}/3
                </span>
              )}
            </div>
            <h2 className="mt-1.5 break-words text-lg font-black leading-tight tracking-[-0.02em] sm:text-xl">
              {task.title}
            </h2>
          </div>
          <button
            onClick={close}
            aria-label="Close"
            className="grid size-9 shrink-0 place-items-center border-2 border-ink bg-contrast shadow-brutal-sm hover:bg-emphasis focus-visible:outline-4"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path
                d="M4 4l8 8M12 4l-8 8"
                stroke="currentColor"
                strokeWidth="2.5"
                strokeLinecap="square"
              />
            </svg>
          </button>
        </header>

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-3 space-y-4 sm:p-4">
          {/* Phase 9: why this task left REVIEW without the human's diff
              landing — shown while the auto-merge agent runs. */}
          {task.status === "RESOLVING_CONFLICTS" && (
            <section className="border-4 border-ink bg-field px-3 py-2.5 text-[13px] font-medium leading-relaxed text-ink shadow-brutal-sm">
              <strong className="font-black">Merge conflict detected.</strong>{" "}
              The repository changed while this task was running, so the
              approved diff no longer applied. Its workspace has been rebased
              onto the current code with conflict markers and an agent is
              reconciling them now — the result returns here for a second,
              clean approval. Nothing was written to your working tree.
            </section>
          )}

          {task.description.trim() && (
            <section>
              <h3 className={`${LABEL} mb-1.5`}>Description</h3>
              <p className="text-[14px] font-medium leading-relaxed whitespace-pre-wrap">
                {task.description}
              </p>
            </section>
          )}

          {/* Diff / logs tabs */}
          <section>
            <div className="flex gap-1 mb-2">
              {(["diff", "logs"] as const).map((t) => (
                <button
                  key={t}
                  onClick={() => setTab(t)}
                  aria-pressed={tab === t}
                  className={`border-2 border-ink px-3 py-1 font-mono text-[11px] font-bold uppercase tracking-[0.08em] ${
                    tab === t
                      ? "bg-ink text-emphasis"
                      : "bg-contrast text-ink hover:bg-emphasis"
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
              <label htmlFor="feedback" className={LABEL}>
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
                className="w-full border-[3px] border-ink bg-contrast px-3 py-2 text-sm font-medium text-ink shadow-brutal-sm placeholder:text-ink/40 resize-y"
              />
            </section>
          )}
        </div>

        {/* Failures surface here as well as in the page banner: this dialog
            is a full-screen overlay, so the banner alone would be hidden at
            exactly the moment it matters (a merge conflict on Approve). */}
        {error && (
          <div className="px-3 pb-3 sm:px-4">
            <div
              role="alert"
              className="flex flex-wrap items-center gap-2 border-4 border-ink bg-signal-ink px-3 py-2 text-[12px] font-bold leading-relaxed text-contrast break-words"
            >
              <span className="flex-1">{error}</span>
              <button
                onClick={clearError}
                className="border-2 border-ink bg-ink px-2 py-0.5 text-[10px] font-black uppercase tracking-[0.08em] text-emphasis hover:bg-contrast hover:text-ink focus-visible:outline-contrast"
              >
                dismiss
              </button>
            </div>
          </div>
        )}

        {/* Footer actions */}
        <footer className="flex flex-wrap items-center gap-2 border-t-4 border-ink px-3 py-3 sm:px-4">
          {isReview ? (
            <>
              <button
                onClick={() => void approve(task.id)}
                disabled={busy}
                className={`${ACTION} bg-emphasis`}
              >
                Approve
              </button>
              <button
                onClick={() => void reject(task.id, feedback)}
                disabled={busy}
                className={`${ACTION} bg-signal`}
              >
                Reject
              </button>
            </>
          ) : isTodo ? (
            <button
              onClick={() => void runOne(task.id)}
              disabled={busy}
              className={`${ACTION} bg-signal`}
            >
              Run agent
            </button>
          ) : isFailed ? (
            <button
              onClick={() => void reject(task.id, feedback)}
              disabled={busy}
              title="Send the parked task back to the queue with a fresh retry budget (workspace kept)."
              className={`${ACTION} bg-field`}
            >
              Send back to queue
            </button>
          ) : isWorkerOwned ? (
            <button
              onClick={() => void recover(task.id)}
              disabled={busy}
              title="Worker died or was interrupted? Free its claim so the task can run again (workspace kept)."
              className={`${ACTION} bg-contrast`}
            >
              Release stuck task
            </button>
          ) : null}
          <button
            onClick={close}
            className={`${ACTION} ml-auto bg-contrast`}
          >
            Close
          </button>
        </footer>
      </div>
    </div>
  );
}
