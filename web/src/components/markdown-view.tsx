"use client";

import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * Render the agent's final response as Markdown.
 *
 * The transcript stays in the Logs tab; this view is deliberately separate so
 * an analysis task that changes no files still has a readable result. Raw HTML
 * is not enabled — React escapes it, which is the right default for model
 * output.
 */
export default function MarkdownView({ markdown }: { markdown: string }) {
  return (
    <div className="markdown-body border-[3px] border-ink bg-contrast p-3 text-[14px] leading-relaxed overflow-auto max-h-80 min-h-[4rem]">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{markdown}</ReactMarkdown>
    </div>
  );
}
