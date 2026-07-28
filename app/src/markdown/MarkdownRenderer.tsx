import { Children, Component, useMemo, type ErrorInfo, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import { rehypePlugins, remarkPlugins } from "./rendererConfig";
import { externalHttpUrl, localMediaAttachmentId, markdownUrlTransform } from "./urlPolicy";

interface MarkdownRendererProps {
  onOpenExternalUrl?: (url: string) => Promise<void> | void;
  source: string;
}

export function MarkdownRenderer({ onOpenExternalUrl, source }: MarkdownRendererProps) {
  const components = useMemo(() => rendererComponents(onOpenExternalUrl), [onOpenExternalUrl]);

  return (
    <MarkdownErrorBoundary key={source} source={source}>
      <div className="kosh-markdown">
        <ReactMarkdown
          components={components}
          rehypePlugins={rehypePlugins}
          remarkPlugins={remarkPlugins}
          urlTransform={markdownUrlTransform}
        >
          {source}
        </ReactMarkdown>
      </div>
    </MarkdownErrorBoundary>
  );
}

function rendererComponents(
  onOpenExternalUrl: ((url: string) => Promise<void> | void) | undefined,
): Components {
  return {
    a({ children, href }) {
      const url = externalHttpUrl(href);
      if (!url || !onOpenExternalUrl) {
        return <span className="kosh-markdown__inert-link">{children}</span>;
      }
      const openLink = () => {
        Promise.resolve(onOpenExternalUrl(url)).catch((error: unknown) => {
          console.error("Could not open external Markdown link", error);
        });
      };
      return (
        <button
          className="kosh-markdown__external-link"
          onClick={openLink}
          role="link"
          title={url}
          type="button"
        >
          {children}
        </button>
      );
    },
    img({ alt, src, title }) {
      const attachmentId = localMediaAttachmentId(src);
      const metadata = parseImageTitle(title);
      if (attachmentId && metadata) {
        return (
          <figure className="kosh-markdown__image" style={{ width: `${metadata.widthPercent}%` }}>
            <img alt={alt ?? ""} src={src} />
            {metadata.caption && <figcaption>{metadata.caption}</figcaption>}
          </figure>
        );
      }
      return (
        <span className="kosh-markdown__inert-image">
          {alt ? `Image: ${alt}` : "External image unavailable"}
        </span>
      );
    },
    p({ children }) {
      const values = Children.toArray(children);
      return <p>{values}</p>;
    },
    table({ children }) {
      return (
        <div className="kosh-markdown__table-scroll" tabIndex={0}>
          <table>{children}</table>
        </div>
      );
    },
  };
}

function parseImageTitle(
  value: string | undefined,
): { caption: string; widthPercent: number } | null {
  const match = /^kosh-image:(100|[1-9][0-9]):(.*)$/u.exec(value ?? "");
  if (!match) {
    return null;
  }
  const widthPercent = Number(match[1]);
  if (widthPercent < 10) {
    return null;
  }
  try {
    return { caption: decodeURIComponent(match[2]!), widthPercent };
  } catch {
    return null;
  }
}

interface ErrorBoundaryProps {
  children: ReactNode;
  source: string;
}

interface ErrorBoundaryState {
  failed: boolean;
}

class MarkdownErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { failed: false };

  static getDerivedStateFromError(): ErrorBoundaryState {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Kosh Markdown rendering failed", error, info);
  }

  render() {
    if (this.state.failed) {
      return (
        <div
          aria-label="Tidbit content could not be rendered"
          className="kosh-markdown kosh-markdown--error"
          role="note"
        >
          <p>Formatting failed. Showing the original source.</p>
          <pre>{this.props.source}</pre>
        </div>
      );
    }
    return this.props.children;
  }
}
