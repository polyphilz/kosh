import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

Object.defineProperty(window, "scrollTo", {
  configurable: true,
  value: () => undefined,
});

Object.defineProperty(window.navigator, "platform", {
  configurable: true,
  value: "MacIntel",
});

Object.defineProperty(window, "matchMedia", {
  configurable: true,
  value: (query: string): MediaQueryList => ({
    addEventListener: () => undefined,
    addListener: () => undefined,
    dispatchEvent: () => false,
    matches: false,
    media: query,
    onchange: null,
    removeEventListener: () => undefined,
    removeListener: () => undefined,
  }),
});

globalThis.ResizeObserver = class ResizeObserver {
  disconnect() {}
  observe() {}
  unobserve() {}
};

Object.defineProperty(SVGElement.prototype, "getBBox", {
  configurable: true,
  value: () => new DOMRect(),
});

Object.defineProperty(globalThis, "CSS", {
  configurable: true,
  value: { supports: () => true },
});

if (!globalThis.ClipboardEvent) {
  Object.defineProperty(globalThis, "ClipboardEvent", {
    configurable: true,
    value: class ClipboardEvent extends Event {
      readonly clipboardData = null;
    },
  });
}

if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
}

if (!Range.prototype.getBoundingClientRect) {
  Range.prototype.getBoundingClientRect = () => new DOMRect();
}

HTMLElement.prototype.scrollIntoView = () => undefined;
document.elementFromPoint = () => null;

afterEach(cleanup);
