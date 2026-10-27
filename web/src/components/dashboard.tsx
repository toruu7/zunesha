"use client";

import { useEffect } from "react";
import dynamic from "next/dynamic";

import Board from "@/components/board";
import PlanBar from "@/components/plan-bar";
import ReviewModal from "@/components/review-modal";
import { useZou } from "@/lib/store";

// reactflow measures the DOM (and needs `window`), so the graph view loads
// client-side only. The code split also keeps it — and its stylesheet — out
// of the initial bundle until the view is switched on.
const DagView = dynamic(() => import("@/components/dag-view"), { ssr: false });

const STATUS_FILL: Record<string, string> = {
  open: "bg-ink",
  connecting: "bg-emphasis animate-pulse motion-reduce:animate-none",
  closed: "bg-contrast",
};

function ConnectionDot() {
  const connection = useZou((s) => s.connection);
  const label =
    connection === "open"
      ? "connected"
      : connection === "connecting"
        ? "connecting…"
        : "disconnected — retrying";
  return (
    <span
      className="flex items-center gap-1.5 text-[11px] font-bold uppercase tracking-[0.06em]"
      title={`daemon ${label}`}
    >
      <span
        className={`size-3 shrink-0 border-2 border-ink ${STATUS_FILL[connection]}`}
      />
      {label}
    </span>
  );
}

function ErrorBanner() {
  const error = useZou((s) => s.error);
  const clearError = useZou((s) => s.clearError);
  if (!error) return null;
  return (
    <div
      role="alert"
      className="flex items-start gap-3 border-4 border-ink bg-signal-ink px-3 py-2 text-[13px] font-bold leading-snug text-contrast shadow-brutal-sm"
    >
      <span className="flex-1 break-all">{error}</span>
      <button
        onClick={clearError}
        className="shrink-0 border-2 border-ink bg-ink px-2 py-1 text-[10px] font-black uppercase tracking-[0.08em] text-emphasis hover:bg-contrast hover:text-ink focus-visible:outline-contrast"
      >
        dismiss
      </button>
    </div>
  );
}

/** Kanban board ⇄ interactive DAG graph (Phase 11). */
function ViewToggle() {
  const view = useZou((s) => s.view);
  const setView = useZou((s) => s.setView);
  return (
    <div
      className="flex items-center gap-1 border-4 border-ink bg-contrast p-1 shadow-brutal-sm"
      role="tablist"
      aria-label="board view"
    >
      {(["board", "dag"] as const).map((v) => (
        <button
          key={v}
          role="tab"
          aria-selected={view === v}
          onClick={() => setView(v)}
          className={`border-2 border-ink px-2.5 py-1 text-[10px] font-black uppercase tracking-[0.1em] ${
            view === v
              ? "bg-ink text-emphasis"
              : "bg-contrast text-ink hover:bg-emphasis"
          }`}
        >
          {v}
        </button>
      ))}
    </div>
  );
}

export default function Dashboard() {
  const connect = useZou((s) => s.connect);
  const disconnect = useZou((s) => s.disconnect);
  const view = useZou((s) => s.view);
  const selectedId = useZou((s) => s.selectedId);
  const selectedTask = useZou((s) =>
    s.tasks.find((t) => t.id === s.selectedId),
  );

  // Connect to the daemon on mount; tear down on unmount. `connect` is
  // idempotent, so React StrictMode's double-invoke is safe.
  useEffect(() => {
    connect();
    return disconnect;
  }, [connect, disconnect]);

  return (
    <div className="flex flex-col h-dvh bg-ground text-ink">
      <header className="shrink-0 border-b-4 border-ink">
        {/* Red poster: the large statement strip. */}
        <div className="bg-signal">
          <div className="max-w-[1600px] mx-auto flex flex-wrap items-center gap-x-3 gap-y-2 px-3 py-3 sm:px-4">
            <h1 className="text-5xl font-black uppercase leading-[0.78] tracking-[-0.065em]">
              zou
            </h1>
            <span className="-rotate-2 border-2 border-ink bg-ink px-2 py-1 text-[11px] font-black uppercase tracking-[0.14em] text-emphasis">
              zunesha
            </span>
            {/* Decorative directional tile: the sheet reads top to bottom. */}
            <span
              aria-hidden="true"
              className="hidden size-8 shrink-0 place-items-center border-4 border-ink bg-ink text-emphasis shadow-brutal-sm sm:grid"
            >
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path
                  d="M8 2v10M3.5 7.5 8 12l4.5-4.5"
                  stroke="currentColor"
                  strokeWidth="3"
                  strokeLinecap="square"
                />
              </svg>
            </span>
            <div className="ml-auto flex flex-wrap items-center gap-2 sm:gap-3">
              <ViewToggle />
              <ConnectionDot />
            </div>
          </div>
        </div>
        {/* Blush band: explanation + actions. */}
        <div className="border-t-4 border-ink bg-field">
          <div className="max-w-[1600px] mx-auto flex flex-col gap-2 px-3 py-3 sm:px-4">
            <PlanBar />
            <ErrorBanner />
          </div>
        </div>
      </header>

      {/* The DAG canvas pans itself, so it fills without page scroll; the
          board's columns keep the page scrollbar. */}
      <main
        className={
          view === "dag"
            ? "flex-1 min-h-0 overflow-hidden p-3 sm:p-4"
            : "flex-1 min-h-0 overflow-y-auto p-3 sm:p-4"
        }
      >
        <div className="max-w-[1600px] mx-auto h-full flex flex-col">
          {view === "dag" ? <DagView /> : <Board />}
        </div>
      </main>

      {selectedId && selectedTask && (
        <ReviewModal key={selectedTask.id} task={selectedTask} />
      )}
    </div>
  );
}
