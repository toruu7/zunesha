"use client";

import { useId, useRef } from "react";

import { formatBytes, type AttachmentDraft } from "@/lib/attachments";

interface AttachmentListProps {
  attachments: AttachmentDraft[];
  errors: string[];
  disabled?: boolean;
  onAdd: (files: Iterable<File>) => void | Promise<unknown>;
  onRemove: (index: number) => void;
  compact?: boolean;
}

/**
 * Attach files/images to a prompt. The actual base64 conversion lives in
 * `use-attachments`; this component only owns the picker and the chips.
 */
export default function AttachmentList({
  attachments,
  errors,
  disabled,
  onAdd,
  onRemove,
  compact = false,
}: AttachmentListProps) {
  const inputId = useId();
  const inputRef = useRef<HTMLInputElement>(null);

  return (
    <div className={compact ? "space-y-1.5" : "space-y-2"}>
      <input
        ref={inputRef}
        id={inputId}
        type="file"
        multiple
        className="sr-only"
        disabled={disabled}
        onChange={(event) => {
          const files = event.target.files;
          if (files?.length) void onAdd(files);
          event.target.value = "";
        }}
      />
      <div className="flex flex-wrap items-center gap-1.5">
        <button
          type="button"
          onClick={() => inputRef.current?.click()}
          disabled={disabled}
          className="flex h-9 items-center gap-1.5 border-[3px] border-ink bg-contrast px-2.5 text-[10px] font-black uppercase tracking-[0.08em] shadow-brutal-sm hover:bg-emphasis disabled:border-ink/40 disabled:text-ink/40 disabled:shadow-none"
        >
          <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden="true">
            <path
              d="M10.5 4.5 5 10a2.1 2.1 0 0 0 3 3l6-6a3.5 3.5 0 0 0-5-5L3 8a5 5 0 0 0 7 7l5-5"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="square"
            />
          </svg>
          Attach
        </button>
        <span className="text-[10px] font-bold uppercase tracking-[0.06em] text-ink/55">
          images or files · max {attachments.length ? `${attachments.length}/10` : "10"}
        </span>
      </div>

      {attachments.length > 0 && (
        <ul className="flex flex-wrap gap-1.5" aria-label="Attached files">
          {attachments.map((attachment, index) => (
            <li
              key={`${attachment.name}-${index}`}
              className="flex max-w-full items-center gap-1.5 border-2 border-ink bg-contrast px-1.5 py-1 shadow-brutal-sm"
            >
              {attachment.preview ? (
                // A local data URL; no remote image loader is involved.
                // eslint-disable-next-line @next/next/no-img-element
                <img
                  src={attachment.preview}
                  alt=""
                  className="size-7 shrink-0 border border-ink object-cover"
                />
              ) : (
                <span className="grid size-7 shrink-0 place-items-center border border-ink bg-ground font-mono text-[9px] font-black">
                  FILE
                </span>
              )}
              <span className="min-w-0">
                <span className="block max-w-[10rem] truncate text-[11px] font-bold leading-tight">
                  {attachment.name}
                </span>
                <span className="block text-[9px] font-bold uppercase tracking-[0.06em] text-ink/55">
                  {formatBytes(attachment.size)}
                </span>
              </span>
              <button
                type="button"
                onClick={() => onRemove(index)}
                disabled={disabled}
                aria-label={`Remove ${attachment.name}`}
                className="grid size-6 shrink-0 place-items-center border border-ink bg-ground hover:bg-signal disabled:opacity-40"
              >
                <svg width="10" height="10" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                  <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="2.5" />
                </svg>
              </button>
            </li>
          ))}
        </ul>
      )}

      {errors.length > 0 && (
        <ul role="alert" className="space-y-0.5 text-[11px] font-bold text-signal-ink">
          {errors.map((error) => (
            <li key={error}>{error}</li>
          ))}
        </ul>
      )}
    </div>
  );
}
