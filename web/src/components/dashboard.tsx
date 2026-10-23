"use client";

import { useEffect } from "react";

import Board from "@/components/board";
import PlanBar from "@/components/plan-bar";
import ReviewModal from "@/components/review-modal";
import { useZou } from "@/lib/store";

const STATUS_COLOR: Record<string, string> = {
  open: "bg-emerald-500",
  connecting: "bg-amber-500 animate-pulse",
  closed: "bg-red-500",
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
      className="flex items-center gap-1.5 text-xs text-zinc-400"
      title={`daemon ${label}`}
    >
      <span className={`size-2 rounded-full ${STATUS_COLOR[connection]}`} />
      {label}
    </span>
  );
}

function ErrorBanner() {
  const error = useZou((s) => s.error);
  const clearError = useZou((s) => s.clearError);
  if (!error) return null;
  return (
    <div className="flex items-center gap-3 rounded-md border border-red-800 bg-red-950/60 px-3 py-2 text-sm text-red-200">
      <span className="flex-1 break-all">{error}</span>
      <button
        onClick={clearError}
        className="shrink-0 text-red-400 hover:text-red-200 text-xs font-medium"
      >
        dismiss
      </button>
    </div>
  );
}

export default function Dashboard() {
  const connect = useZou((s) => s.connect);
  const disconnect = useZou((s) => s.disconnect);
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
    <div className="flex flex-col h-dvh bg-zinc-950 text-zinc-100">
      <header className="shrink-0 border-b border-zinc-800 bg-zinc-900/40 px-4 py-3">
        <div className="max-w-[1600px] mx-auto flex flex-col gap-3">
          <div className="flex items-center gap-3">
            <h1 className="text-lg font-semibold tracking-tight">
              zou <span className="text-zinc-500 font-normal">· zunesha</span>
            </h1>
            <div className="ml-auto">
              <ConnectionDot />
            </div>
          </div>
          <PlanBar />
          <ErrorBanner />
        </div>
      </header>

      <main className="flex-1 min-h-0 overflow-y-auto p-4">
        <div className="max-w-[1600px] mx-auto h-full flex flex-col">
          <Board />
        </div>
      </main>

      {selectedId && selectedTask && <ReviewModal task={selectedTask} />}
    </div>
  );
}
