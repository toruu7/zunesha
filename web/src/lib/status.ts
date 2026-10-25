// Per-status presentation shared by the kanban board and the DAG view.
//
// One source of truth for what each status looks like, so the two views can
// never drift apart.

import type { TaskStatus } from "./types";

/** Per-status accent color (Tailwind class) for rails, dots, and borders. */
export const ACCENT: Record<TaskStatus, string> = {
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
export function isActive(status: TaskStatus): boolean {
  return (
    status === "IN_PROGRESS" ||
    status === "EVALUATING" ||
    status === "RESOLVING_CONFLICTS"
  );
}
