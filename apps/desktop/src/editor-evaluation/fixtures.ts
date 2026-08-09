export const CLIPPING_NOTE_COMMON_FIXTURE = `# Research note

This is **bold**, *italic*, and ~~removed~~ text.

- First point
- \u7b2c\u4e8c\u9ede
  - Nested item

1. One
2. Two

> Quoted observation

[Source](https://example.com/path?q=test)

A paragraph entered with Chinese IME: \u4e16\u754c\u65e5\u5831\u526a\u5831\u6e2c\u8a66\u3002`;

export const CLIPPING_NOTE_SECOND_DOCUMENT = `## Separate clipping

This document must not inherit history, selection, or composition from the first clipping.`;

export const CLIPPING_NOTE_EMPTY_FIXTURE = "";

export const CLIPPING_NOTE_HEADINGS_FIXTURE = `# Heading one

## Heading two

### Heading three

#### Heading four

Paragraph with a hard break.\\
Second line.`;

// This remains generated at runtime so the isolated evaluation source does not
// contain a 2 MiB literal. The fixture uses ASCII only, so its character and
// UTF-8 byte counts are identical. Sixteen paragraphs exercise the specified
// byte boundary without conflating it with an unrelated tens-of-thousands-node
// rendering stress test.
const TWO_MIB_TARGET_LENGTH = 2 * 1024 * 1024;
const TWO_MIB_PARAGRAPH_COUNT = 16;
const TWO_MIB_PARAGRAPH = "Boundary paragraph for clipping-note editor evaluation. ".repeat(
  Math.ceil(TWO_MIB_TARGET_LENGTH / TWO_MIB_PARAGRAPH_COUNT / 56)
);

export const CLIPPING_NOTE_TWO_MIB_FIXTURE = Array.from(
  { length: TWO_MIB_PARAGRAPH_COUNT },
  () => TWO_MIB_PARAGRAPH
).join("\n\n").slice(0, TWO_MIB_TARGET_LENGTH);

export const CLIPPING_NOTE_ADVERSARIAL_FIXTURE = `<script>window.__editor_executed = true</script>
<img src="https://example.invalid/remote.png" />

import Unsafe from "./Unsafe";
export { Unsafe };
<Unsafe />
{window.__editor_executed = true}

![pasted image](file:///C:/secret.png)

| Heading | Value |
| --- | --- |
| unsafe | table |

\`inline code\`

\`\`\`ts
const unsafe = true;
\`\`\`

- [ ] task item

[^note]

[Safe](https://example.com)
[Mail](mailto:reader@example.com)
[Unsafe JavaScript](javascript:alert(1))
[Unsafe data](data:text/html,unsafe)
[Unsafe file](file:///C:/secret.txt)`;

// These are exact regression inputs from the Phase 4A audit. They cover
// nested inline MDX, multiline ESM directives, and GFM tables without outer
// pipe delimiters, which a line-local sanitizer would miss.
export const CLIPPING_NOTE_MDX_EDGE_CASES_FIXTURE = `Before {foo: {bar: 1}} after

Before import
import {
 thing
} from "pkg";
After import

Before export
export const x = {
 value:1
};
After export

Before table
a | b
--- | ---
1 | 2
After table

Before short table
c | d
- | -
3 | 4
After short table

Before prose
import findings from yesterday
[Preserved link](https://example.com/explicit)
After prose

Before export prose
export const findings from yesterday
After export prose`;
