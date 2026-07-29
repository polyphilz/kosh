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

export function injectTrustedCitationLinks(
  source: string,
  mentions: GroundedCitationMention[],
  nonce: string,
): { source: string; hrefs: ReadonlyMap<string, number> } {
  const hrefs = new Map<string, number>();
  const candidates = mentions.filter(
    (mention) =>
      Number.isSafeInteger(mention.citationNumber) &&
      mention.citationNumber >= 1 &&
      Number.isSafeInteger(mention.startByte) &&
      Number.isSafeInteger(mention.endByte) &&
      mention.startByte >= 0 &&
      mention.endByte > mention.startByte,
  );
  const requestedOffsets = new Set<number>();
  for (const mention of candidates) {
    requestedOffsets.add(mention.startByte);
    requestedOffsets.add(mention.endByte);
  }
  const stringIndexes = utf8ByteOffsetsToStringIndexes(source, requestedOffsets);
  const replacements = candidates
    .map((mention) => {
      const start = stringIndexes.get(mention.startByte);
      const end = stringIndexes.get(mention.endByte);
      const marker = `【${mention.citationNumber}】`;
      if (start === undefined || end === undefined || source.slice(start, end) !== marker) {
        return null;
      }
      return { start, end, marker, number: mention.citationNumber };
    })
    .filter((mention) => mention !== null)
    .sort((left, right) => right.start - left.start);
  let nextStart = source.length;
  const accepted = [];
  for (const mention of replacements) {
    if (mention.end > nextStart) {
      continue;
    }
    accepted.push(mention);
    nextStart = mention.start;
  }
  accepted.reverse();
  const parts: string[] = [];
  let cursor = 0;
  for (const mention of accepted) {
    const href = `https://kosh.invalid/citation/${nonce}/${mention.number}`;
    parts.push(source.slice(cursor, mention.start), `[${mention.marker}](${href})`);
    hrefs.set(href, mention.number);
    cursor = mention.end;
  }
  parts.push(source.slice(cursor));
  return { source: parts.join(""), hrefs };
}

function utf8ByteOffsetsToStringIndexes(
  source: string,
  requestedOffsets: ReadonlySet<number>,
): ReadonlyMap<number, number> {
  const indexes = new Map<number, number>();
  let byteOffset = 0;
  let stringIndex = 0;
  if (requestedOffsets.has(0)) indexes.set(0, 0);
  while (stringIndex < source.length && indexes.size < requestedOffsets.size) {
    const codePoint = source.codePointAt(stringIndex)!;
    const stringWidth = codePoint > 0xffff ? 2 : 1;
    byteOffset += codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
    stringIndex += stringWidth;
    if (requestedOffsets.has(byteOffset)) indexes.set(byteOffset, stringIndex);
  }
  return indexes;
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
