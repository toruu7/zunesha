"use client";

import { useEffect, useState } from "react";
import dynamic from "next/dynamic";

import Board from "@/components/board";
import LogViewer from "@/components/log-viewer";
import PlanBar from "@/components/plan-bar";
import ReviewModal from "@/components/review-modal";
import Spinner from "@/components/spinner";
import { isActive } from "@/lib/status";
import { useZou } from "@/lib/store";
import type { Task } from "@/lib/types";

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

/** The repository this daemon is serving, always visible under the masthead. */
function ProjectBadge() {
  const project = useZou((s) => s.project);
  return (
    <div
      className="flex min-w-0 flex-1 items-center gap-2 border-2 border-ink bg-contrast px-2 py-1 shadow-brutal-sm sm:flex-none sm:max-w-[36rem]"
      title={project?.root}
    >
      <span className="shrink-0 border border-ink bg-ink px-1.5 py-0.5 text-[9px] font-black uppercase tracking-[0.1em] text-emphasis">
        project
      </span>
      <div className="min-w-0">
        <div className="truncate text-[12px] font-black uppercase leading-tight tracking-[0.03em]">
          {project?.name ?? "connecting…"}
        </div>
        {project && (
          <div className="truncate font-mono text-[10px] font-bold leading-tight text-ink/65">
            {project.root}
          </div>
        )}
      </div>
    </div>
  );
}

/** A conspicuous live strip: running tasks, not just cards parked in a column. */
function RunningNow() {
  const tasks = useZou((s) => s.tasks);
  const select = useZou((s) => s.select);
  const running = tasks.filter((t) => isActive(t.status));

  if (running.length === 0) {
    return (
      <span className="border-2 border-ink bg-contrast px-2 py-1 text-[10px] font-black uppercase tracking-[0.08em] text-ink/60">
        idle
      </span>
    );
  }

  return (
    <div
      role="status"
      aria-live="polite"
      className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5"
    >
      <span className="flex items-center gap-1.5 border-2 border-ink bg-emphasis px-2 py-1 text-[10px] font-black uppercase tracking-[0.08em]">
        <Spinner className="size-3 text-ink" label="tasks running" />
        {running.length} running
      </span>
      {running.map((task) => (
        <button
          key={task.id}
          onClick={() => select(task.id)}
          title={`${task.id} — ${task.title}`}
          className="max-w-[18rem] truncate border-2 border-ink bg-contrast px-2 py-1 text-left text-[10px] font-bold hover:bg-emphasis focus-visible:outline-4"
        >
          <span className="font-mono uppercase">{task.id}</span>{" "}
          {task.title}
        </button>
      ))}
    </div>
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

/** A bottom dock that keeps the selected/running task's transcript visible. */
function LiveLogsDock({ task, onHide }: { task: Task; onHide: () => void }) {
  const select = useZou((s) => s.select);
  const live = isActive(task.status);
  return (
    <section
      aria-label="Live task logs"
      className="shrink-0 border-t-4 border-ink bg-ground"
    >
      <div className="mx-auto max-w-[1600px] px-3 py-2 sm:px-4">
        <div className="mb-1.5 flex flex-wrap items-center gap-2">
          <span className="flex items-center gap-1 border-2 border-ink bg-ink px-2 py-0.5 text-[10px] font-black uppercase tracking-[0.1em] text-emphasis">
            {live && <Spinner className="size-2.5" label="task running" />}
            {live ? "live logs" : "logs"}
          </span>
          <code className="border border-ink bg-contrast px-1 font-mono text-[10px] font-bold uppercase">
            {task.id}
          </code>
          <span className="min-w-0 flex-1 truncate text-[12px] font-bold">
            {task.title}
          </span>
          <span className="border-2 border-ink bg-contrast px-1.5 py-0.5 text-[10px] font-black uppercase tracking-[0.06em]">
            {task.status.replaceAll("_", " ")}
          </span>
          <button
            onClick={() => select(task.id)}
            className="border-2 border-ink bg-emphasis px-2 py-0.5 text-[10px] font-black uppercase tracking-[0.06em] hover:bg-contrast focus-visible:outline-4"
          >
            open task
          </button>
          <button
            onClick={onHide}
            aria-label="Hide live logs"
            className="grid size-6 place-items-center border-2 border-ink bg-contrast hover:bg-emphasis focus-visible:outline-4"
          >
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
              <path
                d="M4 4l8 8M12 4l-8 8"
                stroke="currentColor"
                strokeWidth="2.5"
                strokeLinecap="square"
              />
            </svg>
          </button>
        </div>
        <LogViewer taskId={task.id} maxHeightClass="max-h-48" />
      </div>
    </section>
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
  const activeTask = useZou((s) => {
    let latest: Task | undefined;
    for (const task of s.tasks) {
      if (!isActive(task.status)) continue;
      if (!latest || task.updated_at > latest.updated_at) latest = task;
    }
    return latest;
  });
  const [logsOpen, setLogsOpen] = useState(false);
  const [lastActiveId, setLastActiveId] = useState<string | null>(null);
  const logTask = selectedTask ?? activeTask ?? null;
  const activeId = activeTask?.id ?? null;

  // Open the dock when a new task is claimed, but never force it open on a
  // mere selection: the review modal already owns that interaction. Adjusting
  // state during render avoids an effect-driven cascading update.
  if (activeId !== lastActiveId) {
    setLastActiveId(activeId);
    if (activeId) {
      setLogsOpen(true);
    } else if (!selectedTask) {
      setLogsOpen(false);
    }
  }

  const toggleLogs = () => setLogsOpen((open) => !open);

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
              <button
                onClick={toggleLogs}
                disabled={!logTask}
                aria-pressed={logsOpen}
                title={
                  logTask
                    ? `${logsOpen ? "Hide" : "Show"} logs for ${logTask.id}`
                    : "No task selected or running"
                }
                className={`flex h-10 items-center gap-2 border-4 border-ink px-2.5 text-[10px] font-black uppercase tracking-[0.1em] shadow-brutal-sm disabled:border-ink/40 disabled:text-ink/40 disabled:shadow-none ${
                  logsOpen
                    ? "bg-ink text-emphasis"
                    : "bg-contrast text-ink hover:bg-emphasis"
                }`}
              >
                {logTask && isActive(logTask.status) && (
                  <Spinner className="size-2.5" label="task running" />
                )}
                <span className="hidden sm:inline">
                  {logTask && isActive(logTask.status) ? "live logs" : "logs"}
                </span>
                <span className="sm:hidden">logs</span>
              </button>
              <ViewToggle />
              <ConnectionDot />
            </div>
          </div>
        </div>
        {/* Blush band: explanation + actions. */}
        <div className="border-t-4 border-ink bg-field">
          <div className="max-w-[1600px] mx-auto flex flex-col gap-2 px-3 py-3 sm:px-4">
            <div className="flex flex-wrap items-center gap-2">
              <ProjectBadge />
              <RunningNow />
            </div>
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

      {logsOpen && logTask && (
        <LiveLogsDock task={logTask} onHide={() => setLogsOpen(false)} />
      )}

      {selectedId && selectedTask && (
        <ReviewModal key={selectedTask.id} task={selectedTask} />
      )}
    </div>
  );
}
