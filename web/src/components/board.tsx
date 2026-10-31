"use client";

import Spinner from "@/components/spinner";
import { ACCENT, isActive } from "@/lib/status";
import { useZou } from "@/lib/store";
import { STATUSES, type Task, type TaskStatus } from "@/lib/types";

function crewEmoji(member?: string): string {
  if (!member) return "";
  const lower = member.toLowerCase();
  if (lower.includes("luffy")) return "👒";
  if (lower.includes("zoro")) return "⚔️";
  if (lower.includes("nami")) return "🗺️";
  if (lower.includes("usopp")) return "🎯";
  if (lower.includes("sanji")) return "🍳";
  return "🏴‍☠️";
}

function TaskCard({ id }: { id: string }) {
  const task = useZou((s) => s.tasks.find((t) => t.id === id));
  const select = useZou((s) => s.select);
  if (!task) return null;

  return (
    <button
      onClick={() => select(task.id)}
      className="group w-full text-left border-[3px] border-ink bg-contrast shadow-brutal-sm hover:-translate-x-0.5 hover:-translate-y-0.5 hover:shadow-brutal active:translate-x-0 active:translate-y-0 active:shadow-brutal-sm focus-visible:outline-4"
    >
      <div className="flex">
        <div className={`w-2 shrink-0 border-r-[3px] border-ink ${ACCENT[task.status]}`} />
        <div className="p-2.5 min-w-0">
          <div className="flex items-center gap-1.5 flex-wrap">
            <code className="shrink-0 border border-ink bg-ground px-1 font-mono text-[10px] font-bold uppercase">
              {task.id}
            </code>
            {task.crew_member && (
              <span className="shrink-0 border border-ink bg-emphasis/40 px-1 text-[10px] font-bold">
                {crewEmoji(task.crew_member)} {task.crew_member}
              </span>
            )}
            {isActive(task.status) && (
              <span className="flex items-center gap-1 text-[9px] font-black uppercase tracking-[0.08em]">
                <Spinner
                  className="size-3 text-signal-ink"
                  label={`${task.id} is running`}
                />
                running
              </span>
            )}
          </div>
          <p className="text-[13px] font-bold leading-snug mt-1 line-clamp-2 break-words">
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

  // Desktop is a fixed 4×2 sheet: four status columns per row, two rows.
  // Phones get a horizontal snap rail (one readable column at a time), while
  // tablets use two columns. The desktop sheet stays unchanged at `lg`.
  return (
    <div className="flex snap-x snap-mandatory gap-3 overflow-x-auto pb-2 sm:grid sm:grid-cols-2 sm:overflow-visible sm:pb-0 lg:grid-cols-4 lg:grid-rows-2 lg:auto-rows-fr lg:flex-1 lg:min-h-0">
      {STATUSES.map((status) => {
        const column = tasks.filter((t) => t.status === status);
        return (
          <section
            key={status}
            className="flex w-[84vw] max-w-sm shrink-0 snap-start flex-col border-4 border-ink bg-contrast shadow-brutal-sm min-h-[12rem] sm:w-auto sm:max-w-none"
          >
            <header className="flex items-center gap-2 border-b-4 border-ink bg-ink px-2.5 py-2">
              <span
                className={`size-3 shrink-0 border-2 border-contrast ${ACCENT[status]}`}
              />
              <h2 className="text-[11px] font-black uppercase tracking-[0.08em] text-contrast">
                {status.replaceAll("_", " ")}
              </h2>
              {isActive(status) && counts[status] > 0 && (
                <Spinner
                  className="size-2.5 text-contrast"
                  label={`${counts[status]} task${counts[status] === 1 ? "" : "s"} running`}
                />
              )}
              <span className="ml-auto border-2 border-ink bg-emphasis px-1.5 py-0.5 text-[10px] font-black text-ink">
                {counts[status]}
              </span>
            </header>
            <div className="flex flex-col gap-2 p-2 overflow-y-auto flex-1">
              {column.length === 0 ? (
                <p className="px-1 py-2 text-[10px] font-black uppercase tracking-[0.12em] text-ink/60">
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
