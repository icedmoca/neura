import { useEffect, useMemo, useState, isValidElement, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";

type MarkdownMessageProps = {
  text: string;
  active?: boolean;
};

function languageLabel(className: string | undefined): string | null {
  const match = /language-([\w+#.-]+)/.exec(className ?? "");
  return match?.[1] ?? null;
}

function childLanguage(children: ReactNode): string | null {
  const first = Array.isArray(children) ? children[0] : children;
  if (!isValidElement<{ className?: string }>(first)) return null;
  return languageLabel(first.props.className);
}

export default function MarkdownMessage({ text, active = false }: MarkdownMessageProps) {
  const [shownLen, setShownLen] = useState(text.length);

  useEffect(() => {
    if (!active) {
      setShownLen(text.length);
      return;
    }
    const timer = window.setInterval(() => {
      setShownLen((prev) => {
        if (prev >= text.length) return prev;
        const gap = text.length - prev;
        const stride = gap > 120 ? 6 : gap > 40 ? 3 : gap > 12 ? 2 : 1;
        return Math.min(text.length, prev + stride);
      });
    }, 18);
    return () => window.clearInterval(timer);
  }, [active, text]);

  const display = active ? text.slice(0, shownLen) : text;

  const components = useMemo<Components>(
    () => ({
      pre({ children, ...props }) {
        const lang = childLanguage(children);
        return (
          <div className="md-code-block">
            {lang ? <div className="md-code-lang">{lang}</div> : null}
            <pre className="md-pre" {...props}>
              {children}
            </pre>
          </div>
        );
      },
      code({ className, children, ...props }) {
        const lang = languageLabel(className);
        const body = String(children);
        const isBlock = Boolean(lang) || body.includes("\n");
        if (!isBlock) {
          return (
            <code className="md-inline-code" {...props}>
              {children}
            </code>
          );
        }
        return (
          <code className={className} {...props}>
            {children}
          </code>
        );
      },
      table({ children }) {
        return (
          <div className="md-table-wrap">
            <table className="md-table">{children}</table>
          </div>
        );
      },
      a({ href, children }) {
        return (
          <a href={href} target="_blank" rel="noopener noreferrer">
            {children}
          </a>
        );
      },
    }),
    [],
  );

  if (!display) return <em className="muted">…</em>;

  return (
    <div className="msg__markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeHighlight]}
        components={components}
      >
        {display}
      </ReactMarkdown>
      {active && shownLen < text.length ? (
        <span className="stream-cursor" aria-hidden>
          ▍
        </span>
      ) : null}
    </div>
  );
}
