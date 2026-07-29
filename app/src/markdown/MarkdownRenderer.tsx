import { Children, Component, useMemo, useRef, type ErrorInfo, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import type { GroundedCitationMention } from "../backend/contracts";
import { inertRemarkPlugins, rehypePlugins, remarkPlugins } from "./rendererConfig";
import { externalHttpUrl, localMediaAttachmentId, markdownUrlTransform } from "./urlPolicy";

interface MarkdownRendererProps {
  allowLocalMedia?: boolean;
  citationMentions?: GroundedCitationMention[];
  onOpenCitation?: (citationNumber: number) => void;
  onOpenExternalUrl?: (url: string) => Promise<void> | void;
  source: string;
}

export function MarkdownRenderer({
  allowLocalMedia = true,
  citationMentions = [],
  onOpenCitation,
  onOpenExternalUrl,
  source,
}: MarkdownRendererProps) {
  const nonce = useRef<string>(globalThis.crypto.randomUUID()).current;
  const trustedCitations = useMemo(
    () => injectTrustedCitationLinks(source, citationMentions, nonce),
    [citationMentions, nonce, source],
  );
  const components = useMemo(
    () =>
      rendererComponents(
        onOpenExternalUrl,
        onOpenCitation,
        trustedCitations.hrefs,
        allowLocalMedia,
      ),
    [allowLocalMedia, onOpenCitation, onOpenExternalUrl, trustedCitations.hrefs],
  );

  return (
    <MarkdownErrorBoundary key={source} source={source}>
      <div className="kosh-markdown">
        <ReactMarkdown
          components={components}
          rehypePlugins={rehypePlugins}
          remarkPlugins={allowLocalMedia ? remarkPlugins : inertRemarkPlugins}
          urlTransform={markdownUrlTransform}
        >
          {trustedCitations.source}
        </ReactMarkdown>
      </div>
    </MarkdownErrorBoundary>
  );
}

function rendererComponents(
  onOpenExternalUrl: ((url: string) => Promise<void> | void) | undefined,
  onOpenCitation: ((citationNumber: number) => void) | undefined,
  trustedCitationHrefs: ReadonlyMap<string, number>,
  allowLocalMedia: boolean,
): Components {
  return {
    a({ children, href }) {
      const citationNumber = href ? trustedCitationHrefs.get(href) : undefined;
      if (citationNumber !== undefined && onOpenCitation) {
        return (
          <button
            aria-label={`Open citation ${citationNumber}`}
            className="kosh-markdown__citation"
            onClick={() => onOpenCitation(citationNumber)}
            type="button"
          >
            {children}
          </button>
        );
      }
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
      const attachmentId = allowLocalMedia ? localMediaAttachmentId(src) : null;
      if (attachmentId && title === "kosh-pdf") {
        return (
          <figure className="kosh-markdown__pdf">
            <object aria-label="PDF attachment preview" data={src} type="application/pdf">
              <span>PDF preview unavailable</span>
            </object>
            <figcaption>{alt ?? "PDF attachment"}</figcaption>
          </figure>
        );
      }
      if (attachmentId && title === "kosh-attachment") {
        return (
          <a className="kosh-markdown__attachment" download href={src}>
            {alt ?? "Attachment"}
          </a>
        );
      }
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

function injectTrustedCitationLinks(
  source: string,
  mentions: GroundedCitationMention[],
  nonce: string,
): { source: string; hrefs: ReadonlyMap<string, number> } {
  const encoder = new TextEncoder();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  const bytes = encoder.encode(source);
  const hrefs = new Map<string, number>();
  const replacements = mentions
    .map((mention) => {
      if (
        !Number.isSafeInteger(mention.citationNumber) ||
        mention.citationNumber < 1 ||
        !Number.isSafeInteger(mention.startByte) ||
        !Number.isSafeInteger(mention.endByte) ||
        mention.startByte < 0 ||
        mention.endByte <= mention.startByte ||
        mention.endByte > bytes.length
      ) {
        return null;
      }
      try {
        const start = decoder.decode(bytes.slice(0, mention.startByte)).length;
        const end = decoder.decode(bytes.slice(0, mention.endByte)).length;
        const marker = `【${mention.citationNumber}】`;
        if (
          source.slice(start, end) !== marker ||
          encoder.encode(source.slice(0, start)).length !== mention.startByte ||
          encoder.encode(source.slice(0, end)).length !== mention.endByte
        ) {
          return null;
        }
        return { start, end, marker, number: mention.citationNumber };
      } catch {
        return null;
      }
    })
    .filter((mention) => mention !== null)
    .sort((left, right) => right.start - left.start);
  let transformed = source;
  let nextStart = source.length;
  for (const mention of replacements) {
    if (mention.end > nextStart) {
      continue;
    }
    const href = `https://kosh.invalid/citation/${nonce}/${mention.number}`;
    transformed =
      transformed.slice(0, mention.start) +
      `[${mention.marker}](${href})` +
      transformed.slice(mention.end);
    hrefs.set(href, mention.number);
    nextStart = mention.start;
  }
  return { source: transformed, hrefs };
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
