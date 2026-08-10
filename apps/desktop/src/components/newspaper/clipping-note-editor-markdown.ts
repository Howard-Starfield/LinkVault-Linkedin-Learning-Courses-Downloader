const SAFE_EXTERNAL_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);
const MAX_UNSUPPORTED_BLOCK_LINES = 256;
const MAX_RAW_TAG_CHARACTERS = 8_192;

/**
 * V1 permits only explicit user-facing web and email links. The editor never
 * opens a link itself; a later, user-activated Tauri adapter must validate
 * again before it delegates to the system opener.
 */
export function isSafeClippingNoteLink(value: string) {
  try {
    return SAFE_EXTERNAL_PROTOCOLS.has(new URL(value).protocol);
  } catch {
    return false;
  }
}

function normalizeMarkdownLink(_match: string, label: string, url: string) {
  return isSafeClippingNoteLink(url) ? `[${label}](${url})` : label;
}

function isFenceStart(line: string) {
  const match = line.match(/^ {0,3}(`{3,}|~{3,})/);
  if (!match) return null;
  return { character: match[1][0], length: match[1].length };
}

function isFenceEnd(line: string, fence: NonNullable<ReturnType<typeof isFenceStart>>) {
  const trimmed = line.trimStart();
  let length = 0;
  while (trimmed[length] === fence.character) length += 1;
  return length >= fence.length;
}

function isMdxImportStart(lines: string[], start: number) {
  if (!/^import\b/.test(lines[start].trimStart())) return false;

  // An ordinary prose line may begin with “import”. Only classify it as ESM
  // when the bounded block includes a quoted module specifier.
  const limit = Math.min(lines.length, start + MAX_UNSUPPORTED_BLOCK_LINES);
  for (let lineIndex = start; lineIndex < limit; lineIndex += 1) {
    const line = lines[lineIndex];
    if (lineIndex > start && line.trim() === "") return false;
    if (/\bfrom\s+["'][^"']+["']\s*;?\s*$/.test(line)) return true;
    if (lineIndex === start && /^import\s*["'][^"']+["']\s*;?\s*$/.test(line.trim())) return true;
  }

  return false;
}

function isMdxExportStart(lines: string[], start: number) {
  const trimmed = lines[start].trimStart();
  if (!/^export\b/.test(trimmed)) return false;
  if (/^export\s+(?:default\b|\{|\*)/.test(trimmed)) return true;
  if (!/^export\s+(?:(?:async\s+)?(?:class|function)|const|enum|interface|let|type|var)\b/.test(trimmed)) {
    return false;
  }

  // Keep prose such as “export const findings from yesterday”. A declaration
  // needs an assignment, terminator, parameter list, or body before it is
  // treated as removable MDX/ESM.
  const limit = Math.min(lines.length, start + MAX_UNSUPPORTED_BLOCK_LINES);
  for (let lineIndex = start; lineIndex < limit; lineIndex += 1) {
    const line = lines[lineIndex];
    if (lineIndex > start && line.trim() === "") return false;
    if (/[=;({]/.test(line)) return true;
  }

  return false;
}

function isMdxDirectiveStart(lines: string[], start: number) {
  return isMdxImportStart(lines, start) || isMdxExportStart(lines, start);
}

function isImportStatementComplete(line: string) {
  return /^(?:import\s+)?(?:["'][^"']+["']|.*\bfrom\s+["'][^"']+["'])\s*;?$/.test(line.trim());
}

function isExportStatementComplete(line: string, isFirstLine: boolean) {
  const trimmed = line.trim();
  if (/[;)}]$/.test(trimmed)) return true;
  return isFirstLine && /^export\s+(?:const|let|var|default)\b/.test(trimmed);
}

/**
 * Scan an MDX ESM block with quote and bracket state rather than relying on a
 * line-local regular expression. The cap makes malformed input unable to
 * consume an unbounded amount of an otherwise valid clipping note.
 */
function endOfMdxDirective(lines: string[], start: number) {
  const startsWithImport = /^import\b/.test(lines[start].trimStart());
  const limit = Math.min(lines.length, start + MAX_UNSUPPORTED_BLOCK_LINES);
  let braces = 0;
  let brackets = 0;
  let parentheses = 0;
  let quote: "'" | '"' | "`" | null = null;
  let escaped = false;
  let inBlockComment = false;

  for (let lineIndex = start; lineIndex < limit; lineIndex += 1) {
    const line = lines[lineIndex];

    for (let characterIndex = 0; characterIndex < line.length; characterIndex += 1) {
      const character = line[characterIndex];
      const nextCharacter = line[characterIndex + 1];

      if (inBlockComment) {
        if (character === "*" && nextCharacter === "/") {
          inBlockComment = false;
          characterIndex += 1;
        }
        continue;
      }

      if (quote) {
        if (escaped) {
          escaped = false;
        } else if (character === "\\") {
          escaped = true;
        } else if (character === quote) {
          quote = null;
        }
        continue;
      }

      if (character === "/" && nextCharacter === "/") break;
      if (character === "/" && nextCharacter === "*") {
        inBlockComment = true;
        characterIndex += 1;
        continue;
      }
      if (character === "'" || character === '"' || character === "`") {
        quote = character;
        continue;
      }
      if (character === "{") braces += 1;
      if (character === "}") braces = Math.max(0, braces - 1);
      if (character === "[") brackets += 1;
      if (character === "]") brackets = Math.max(0, brackets - 1);
      if (character === "(") parentheses += 1;
      if (character === ")") parentheses = Math.max(0, parentheses - 1);

      if (character === ";" && braces === 0 && brackets === 0 && parentheses === 0) {
        return lineIndex;
      }
    }

    if (quote || inBlockComment || braces !== 0 || brackets !== 0 || parentheses !== 0) continue;

    if (startsWithImport && isImportStatementComplete(line)) return lineIndex;
    if (!startsWithImport && isExportStatementComplete(line, lineIndex === start)) return lineIndex;
    if (lineIndex > start && line.trim() === "") return lineIndex - 1;
  }

  return limit - 1;
}

function tableCells(line: string) {
  if (!line.includes("|")) return null;
  let trimmed = line.trim();
  if (trimmed.startsWith("|")) trimmed = trimmed.slice(1);
  if (trimmed.endsWith("|")) trimmed = trimmed.slice(0, -1);
  const cells = trimmed.split("|").map((cell) => cell.trim());
  return cells.length >= 2 && cells.every(Boolean) ? cells : null;
}

function isGfmTableSeparator(cells: string[]) {
  // The installed Markdown parser accepts one or more hyphens here, so the
  // V1 exclusion must cover short GFM delimiter rows as well as `---`.
  return cells.every((cell) => /^:?-+:?$/.test(cell));
}

function gfmTableWidthAt(lines: string[], start: number) {
  const header = tableCells(lines[start]);
  const separator = tableCells(lines[start + 1] ?? "");
  if (!header || !separator || header.length !== separator.length || !isGfmTableSeparator(separator)) return 0;
  return header.length;
}

/**
 * Drop unsupported fenced code, MDX ESM, footnote definitions, and GFM table
 * blocks before Markdown reaches Tiptap. This scanner intentionally accepts
 * both pipe-wrapped and pipe-less GFM tables.
 */
function stripUnsupportedBlocks(markdown: string) {
  const lines = markdown.split("\n");
  const retained: string[] = [];

  for (let lineIndex = 0; lineIndex < lines.length;) {
    const fence = isFenceStart(lines[lineIndex]);
    if (fence) {
      lineIndex += 1;
      while (lineIndex < lines.length && !isFenceEnd(lines[lineIndex], fence)) lineIndex += 1;
      if (lineIndex < lines.length) lineIndex += 1;
      continue;
    }

    if (isMdxDirectiveStart(lines, lineIndex)) {
      lineIndex = endOfMdxDirective(lines, lineIndex) + 1;
      continue;
    }

    const tableWidth = gfmTableWidthAt(lines, lineIndex);
    if (tableWidth > 0) {
      lineIndex += 2;
      while (tableCells(lines[lineIndex] ?? "")?.length === tableWidth) lineIndex += 1;
      continue;
    }

    if (/^\s*\[\^[^\]]+\]:/.test(lines[lineIndex])) {
      lineIndex += 1;
      continue;
    }

    retained.push(lines[lineIndex]);
    lineIndex += 1;
  }

  return retained.join("\n");
}

/**
 * Strip raw HTML and JSX tags without treating text between them as HTML.
 * The scan is bounded, and an incomplete tag is made inert instead of being
 * permitted to survive into persisted Markdown.
 */
function stripRawTags(markdown: string) {
  let result = "";

  for (let index = 0; index < markdown.length;) {
    if (markdown[index] !== "<" || !/[A-Za-z!/]/.test(markdown[index + 1] ?? "")) {
      result += markdown[index];
      index += 1;
      continue;
    }

    if (markdown.startsWith("<!--", index)) {
      const commentEnd = markdown.indexOf("-->", index + 4);
      if (commentEnd !== -1 && commentEnd - index <= MAX_RAW_TAG_CHARACTERS) {
        index = commentEnd + 3;
        continue;
      }
    }

    let quote: "'" | '"' | null = null;
    let escaped = false;
    const limit = Math.min(markdown.length, index + MAX_RAW_TAG_CHARACTERS);
    let cursor = index + 1;
    for (; cursor < limit; cursor += 1) {
      const character = markdown[cursor];
      if (quote) {
        if (escaped) {
          escaped = false;
        } else if (character === "\\") {
          escaped = true;
        } else if (character === quote) {
          quote = null;
        }
        continue;
      }
      if (character === "'" || character === '"') {
        quote = character;
      } else if (character === ">") {
        break;
      } else if (character === "\n") {
        break;
      }
    }

    if (markdown[cursor] === ">") {
      index = cursor + 1;
    } else {
      // Preserve readable prose but remove the leading raw-tag delimiter.
      result += "(";
      index += 1;
    }
  }

  return result;
}

/**
 * MDX expressions can be nested. Convert every delimiter in one pass so an
 * inner replacement can never leave an outer `{...}` expression behind.
 */
function inertifyMdxExpressions(markdown: string) {
  let result = "";
  for (const character of markdown) {
    if (character === "{") result += "(";
    else if (character === "}") result += ")";
    else result += character;
  }
  return result;
}

/**
 * Keep persisted clipping notes within the deliberately small V1 Markdown
 * subset. Tiptap's Markdown parser safely renders unsupported input as text,
 * but that alone would let raw HTML and MDX directives round-trip into stored
 * Markdown. Normalizing both the initial content and serialized output makes
 * unsupported syntax inert and non-persistent without adding an HTML preview
 * or an editor package-specific document format.
 */
export function normalizeClippingNoteMarkdown(markdown: string) {
  let normalized = stripUnsupportedBlocks(markdown.replace(/\r\n?/g, "\n").replace(/\u0000/g, ""));

  // Images become their alt text. Inline code becomes ordinary safe text
  // rather than reintroducing syntax outside the approved editor subset.
  normalized = normalized.replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1");
  normalized = normalized.replace(/(^|[^\\])`([^`]+)`/g, "$1$2");
  normalized = normalized.replace(/\[\^[^\]]+\]/g, "");

  // Retain only the explicit link schemes allowed by the adapter. Any other
  // Markdown link degrades to its readable label before the editor sees it.
  normalized = normalized.replace(
    /\[([^\]]+)]\(([^\s)]+)(?:\s+(?:"[^"]*"|'[^']*'))?\)/g,
    normalizeMarkdownLink
  );

  normalized = inertifyMdxExpressions(stripRawTags(normalized));

  return normalized.replace(/\n{3,}/g, "\n\n").trim();
}
