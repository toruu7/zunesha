"use client";

import { useEffect, useRef } from "react";

import Spinner from "@/components/spinner";
import { useZou } from "@/lib/store";

/**
 * Live technical transcript for one task.
 *
 * Lines come from the store's reducer, which merges streamed `log` events with
 * the persisted `worker_logs` prefix by `seq` — so this view is seamless across
 * reconnects and never duplicates or drops lines. Gaps (a dropped event) show
 * as a visible placeholder rather than silently missing text.
 *
 * The agent's final Markdown answer is shown separately in the Output tab; this
 * view keeps tool calls, stderr, and `[eval]` output for debugging.
 */
export default function LogViewer({
  taskId,
  maxHeightClass = "max-h-64",
}: {
  taskId: string;
  /** Tailwind max-height utility; the dashboard dock uses a shorter pane. */
  maxHeightClass?: string;
}) {
  const lines = useZou((s) => s.logs[taskId]);
  const status = useZou((s) => s.tasks.find((t) => t.id === taskId)?.status);
  const scroller = useRef<HTMLDivElement>(null);

  // Auto-scroll to the newest line, but only if the user is already near the
  // bottom — otherwise respect their place in the output.
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const nearBottom =
      el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [lines?.length]);

  // All worker-owned statuses show live output (EVALUATING streams the
  // test command's output, prefixed `[eval]`; RESOLVING_CONFLICTS streams
  // the auto-merge agent reconciling conflict markers).
  const running =
    status === "IN_PROGRESS" ||
    status === "EVALUATING" ||
    status === "RESOLVING_CONFLICTS";
  const empty = !lines || lines.length === 0;

  return (
    <div className="flex flex-col overflow-hidden border-[3px] border-ink bg-contrast">
      <div className="flex items-center gap-2 border-b-[3px] border-ink bg-ground px-3 py-1.5">
        <span className="text-[11px] font-black uppercase tracking-[0.1em]">
          logs
        </span>
        {running && (
          <span className="flex items-center gap-1 border-2 border-ink bg-emphasis px-1.5 py-0.5 text-[10px] font-black uppercase tracking-[0.06em]">
            <Spinner className="size-2.5 text-ink" label="task running" />
            running
          </span>
        )}
        {!empty && (
          <span className="ml-auto font-mono text-[11px] font-bold text-ink/60">
            {lines.length} line{lines.length === 1 ? "" : "s"}
          </span>
        )}
      </div>
      <div
        ref={scroller}
        className={`p-2.5 font-mono text-[12px] leading-relaxed overflow-auto ${maxHeightClass} min-h-[4rem]`}
      >
        {empty ? (
          <p className="font-medium italic text-ink/60">
            {running ? "waiting for logs…" : "no logs captured"}
          </p>
        ) : (
          lines.map((l) => (
            <div key={l.seq} className="whitespace-pre-wrap break-all">
              {/* A `seq` gap means an event was dropped in transit; surface it
                  instead of silently splicing the neighbouring lines together. */}
              {l.line === "" ? (
                <span>&nbsp;</span>
              ) : (
                <span
                  className={
                    l.stream === "stderr"
                      ? "font-bold text-signal-ink"
                      : "text-ink/85"
                  }
                >
                  {l.line}
                </span>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
