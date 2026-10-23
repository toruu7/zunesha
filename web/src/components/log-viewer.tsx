"use client";

import { useEffect, useRef } from "react";

import { useZou } from "@/lib/store";

/**
 * Live agent output for one task.
 *
 * Lines come from the store's reducer, which merges streamed `log` events with
 * the persisted `worker_logs` prefix by `seq` — so this view is seamless across
 * reconnects and never duplicates or drops lines. Gaps (a dropped event) show
 * as a visible placeholder rather than silently missing text.
 */
export default function LogViewer({ taskId }: { taskId: string }) {
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
    <div className="flex flex-col rounded-md border border-zinc-800 bg-black/60 overflow-hidden">
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-zinc-800 bg-zinc-900/60">
        <span className="text-[11px] font-mono uppercase tracking-wide text-zinc-500">
          output
        </span>
        {running && (
          <span className="flex items-center gap-1 text-[11px] text-amber-400">
            <span className="inline-block size-1.5 rounded-full bg-amber-400 animate-pulse" />
            running
          </span>
        )}
        {!empty && (
          <span className="ml-auto text-[11px] font-mono text-zinc-600">
            {lines.length} line{lines.length === 1 ? "" : "s"}
          </span>
        )}
      </div>
      <div
        ref={scroller}
        className="p-2.5 font-mono text-[12px] leading-relaxed overflow-auto max-h-64 min-h-[4rem]"
      >
        {empty ? (
          <p className="text-zinc-600 italic">
            {running ? "waiting for output…" : "no output captured"}
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
                    l.stream === "stderr" ? "text-red-400" : "text-zinc-300"
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
