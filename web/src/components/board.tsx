"use client";

import { useZou } from "@/lib/store";
import { STATUSES, type Task, type TaskStatus } from "@/lib/types";

// Per-status accent colors for the column header + card left-rail.
const ACCENT: Record<TaskStatus, string> = {
  BACKLOG: "bg-zinc-500",
  TODO: "bg-sky-500",
  IN_PROGRESS: "bg-amber-500",
  EVALUATING: "bg-teal-500",
  REVIEW: "bg-violet-500",
  RESOLVING_CONFLICTS: "bg-orange-500",
  DONE: "bg-emerald-500",
  FAILED_NEEDS_HUMAN: "bg-rose-600",
};

/** Statuses a live worker currently owns (pulsing dot). */
function isActive(status: TaskStatus): boolean {
  return (
    status === "IN_PROGRESS" ||
    status === "EVALUATING" ||
    status === "RESOLVING_CONFLICTS"
  );
}

function TaskCard({ id }: { id: string }) {
  const task = useZou((s) => s.tasks.find((t) => t.id === id));
  const select = useZou((s) => s.select);
  if (!task) return null;

  return (
    <button
      onClick={() => select(task.id)}
      className="group w-full text-left rounded-md border border-zinc-800 bg-zinc-900/60 hover:border-zinc-600 hover:bg-zinc-800/70 transition-colors overflow-hidden"
    >
      <div className="flex">
        <div className={`w-1 shrink-0 ${ACCENT[task.status]}`} />
        <div className="p-2.5 min-w-0">
          <div className="flex items-center gap-1.5">
            <code className="text-[10px] text-zinc-500 font-mono shrink-0">
              {task.id}
            </code>
            {isActive(task.status) && (
              <span className="inline-block size-1.5 rounded-full bg-amber-400 animate-pulse shrink-0" />
            )}
          </div>
          <p className="text-sm text-zinc-100 leading-snug mt-0.5 line-clamp-2 break-words">
            {task.title}
          </p>
        </div>
      </div>
    </button>
  );
}

export default function Board() {
  const tasks = useZou((s) => s.tasks);
  const counts = countsFrom(tasks);

  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-8 gap-3 flex-1 min-h-0">
      {STATUSES.map((status) => {
        const column = tasks.filter((t) => t.status === status);
        return (
          <section
            key={status}
            className="flex flex-col rounded-lg border border-zinc-800 bg-zinc-900/30 min-h-[12rem]"
          >
            <header className="flex items-center gap-2 px-3 py-2 border-b border-zinc-800">
              <span className={`size-2 rounded-full ${ACCENT[status]}`} />
              <h2 className="text-xs font-semibold tracking-wide text-zinc-300">
                {status.replaceAll("_", " ")}
              </h2>
              <span className="ml-auto text-[11px] font-mono text-zinc-500">
                {counts[status]}
              </span>
            </header>
            <div className="flex flex-col gap-2 p-2 overflow-y-auto flex-1">
              {column.length === 0 ? (
                <p className="text-[11px] text-zinc-600 px-1 py-2 italic">
                  empty
                </p>
              ) : (
                column.map((t) => <TaskCard key={t.id} id={t.id} />)
              )}
            </div>
          </section>
        );
      })}
    </div>
  );
}

// Local tally so column headers update without an extra selector per column.
function countsFrom(tasks: Task[]): Record<TaskStatus, number> {
  const out = {
    BACKLOG: 0,
    TODO: 0,
    IN_PROGRESS: 0,
    EVALUATING: 0,
    REVIEW: 0,
    RESOLVING_CONFLICTS: 0,
    DONE: 0,
    FAILED_NEEDS_HUMAN: 0,
  };
  for (const t of tasks) out[t.status] += 1;
  return out;
}
