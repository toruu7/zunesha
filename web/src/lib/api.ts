// REST + WebSocket access to the orchestrator daemon.
//
// Base-URL rules:
//   * Served by the daemon itself (`web/out` via Axum) → same origin, so
//     relative `/api/...` and `ws(s)://<host>/ws` just work.
//   * Under `next dev` (port 3000/3001) the daemon is on `127.0.0.1:57804`;
//     the daemon's CORS is `very_permissive` so cross-origin is fine.
//   * `NEXT_PUBLIC_API_BASE` overrides both (tests, remote daemon).

import type { AttachmentPayload } from "./attachments";
import type {
  ApiEvent,
  ApproveResponse,
  CancelResponse,
  DeleteResponse,
  RecoverResponse,
  StatusResponse,
  Task,
  TasksResponse,
} from "./types";
import { ApiError } from "./types";

/** The HTTP(S) origin of the daemon, or "" for same-origin. */
export function apiBase(): string {
  const override = process.env.NEXT_PUBLIC_API_BASE;
  if (override) return override.replace(/\/+$/, "");
  if (typeof window === "undefined") return "";
  const { port } = window.location;
  if (port === "3000" || port === "3001") {
    return `${window.location.protocol}//${window.location.hostname}:57804`;
  }
  return "";
}

/** The daemon's WebSocket endpoint, matched to {@link apiBase}. */
export function wsUrl(): string {
  const base = apiBase();
  if (base) return base.replace(/^http/, "ws") + "/ws";
  if (typeof window === "undefined") return "ws://127.0.0.1:57804/ws";
  const scheme = window.location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${window.location.host}/ws`;
}

/** Fetch JSON, throwing {@link ApiError} on non-2xx with the server's message. */
async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(apiBase() + path, {
    // Send cookies/headers consistently; the daemon is local-only.
    credentials: "same-origin",
    ...init,
    headers: { "content-type": "application/json", ...init?.headers },
  });

  if (!res.ok) {
    // The API always answers `{ error: string }`; fall back to the status.
    let message = `${res.status} ${res.statusText}`;
    try {
      const body = (await res.json()) as { error?: string };
      if (body?.error) message = body.error;
    } catch {
      // Non-JSON error body — keep the status text.
    }
    throw new ApiError(res.status, message);
  }

  // 204/empty bodies aren't expected from this API, but don't explode if seen.
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

function post<T>(path: string, body?: unknown): Promise<T> {
  return request<T>(path, {
    method: "POST",
    body: JSON.stringify(body ?? {}),
  });
}

function del<T>(path: string): Promise<T> {
  return request<T>(path, {
    method: "DELETE",
  });
}

export const api = {
  tasks: () => request<TasksResponse>("/api/tasks"),
  status: () => request<StatusResponse>("/api/status"),
  task: (id: string) => request<Task>(`/api/tasks/${encodeURIComponent(id)}`),
  plan: (prompt: string, attachments: AttachmentPayload[] = []) =>
    post<{ task_ids: string[] }>("/api/plan", { prompt, attachments }),
  runAll: () => post<{ started: number }>("/api/run"),
  runOne: (id: string) =>
    post<{ started: number }>(`/api/tasks/${encodeURIComponent(id)}/run`),
  cancel: (id: string) =>
    post<CancelResponse>(`/api/tasks/${encodeURIComponent(id)}/cancel`),
  deleteTask: (id: string) =>
    del<DeleteResponse>(`/api/tasks/${encodeURIComponent(id)}`),
  approve: (id: string) =>
    post<ApproveResponse>(`/api/tasks/${encodeURIComponent(id)}/approve`),
  reject: (
    id: string,
    feedback: string,
    attachments: AttachmentPayload[] = [],
  ) =>
    post<Task>(`/api/tasks/${encodeURIComponent(id)}/reject`, {
      feedback,
      attachments,
    }),
  /** Release a stuck IN_PROGRESS task back to TODO (crash recovery). */
  recover: (id: string) =>
    post<RecoverResponse>("/api/recover", { task_id: id }),
};

/**
 * Connect to the daemon's event stream.
 *
 * Returns the socket; the caller owns it (and its lifecycle). `onEvent` fires
 * for every decoded {@link ApiEvent}. Malformed frames are surfaced through
 * `onError` rather than thrown, so one bad message can't tear down the view.
 */
export function connectEvents(
  onEvent: (ev: ApiEvent) => void,
  onError?: (err: unknown) => void,
): WebSocket {
  const socket = new WebSocket(wsUrl());
  socket.addEventListener("message", (msg) => {
    try {
      onEvent(JSON.parse(String(msg.data)) as ApiEvent);
    } catch (err) {
      onError?.(err);
    }
  });
  return socket;
}
