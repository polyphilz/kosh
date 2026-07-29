import { describe, expect, it } from "vitest";
import { markdownToPlainText } from "../../src/markdown/plainText";

describe("markdownToPlainText", () => {
  it("keeps authored meaning while removing Markdown syntax", () => {
    expect(
      markdownToPlainText(
        "# Formula\n\nA **strong** [claim](https://example.com) with $x^2$.\n\n```ts\nconst n = 2;\n```\n\n- one\n- two",
      ),
    ).toBe("Formula\n\nA strong claim with x^2.\n\nconst n = 2;\n\n• one\n• two");
  });

  it("does not leak local attachment capability tokens", () => {
    expect(
      markdownToPlainText(
        "{{kosh:image:019f547b-6200-7000-8000-000000000001;width=70%;alt=Diagram}}\n\n{{kosh:attachment:019f547b-6200-7000-8000-000000000002}}",
      ),
    ).toBe("Diagram\n\nAttachment");
  });
});
