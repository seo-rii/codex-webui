import assert from "node:assert/strict";
import test from "node:test";

import { renderMarkdown } from "../src/lib/markdown-renderer.ts";

test("renders explicit code languages with shared lowlight highlighting", () => {
  const html = renderMarkdown("```js\nconst value = 1;\n```", "Copy code");

  assert.match(html, /language-js/u);
  assert.match(html, /hljs-keyword/u);
  assert.match(html, /data-copy-label="Copy code"/u);
});

test("renders unlabelled code without expensive automatic language detection", () => {
  const html = renderMarkdown("```\nconst value = 1;\n```", "Copy code");

  assert.match(html, /<code class="hljs">const value = 1;/u);
  assert.doesNotMatch(html, /hljs-keyword/u);
});

test("escapes raw html and blocks unsafe links", () => {
  const html = renderMarkdown('<script>alert("x")</script>\n\n[unsafe](javascript:alert(1))', "Copy code");

  assert.doesNotMatch(html, /<script>/u);
  assert.doesNotMatch(html, /href="javascript:/u);
  assert.match(html, /&lt;script&gt;/u);
});
