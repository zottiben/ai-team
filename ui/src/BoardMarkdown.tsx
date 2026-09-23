import type { ReactNode } from "react";

const INLINE =
  /(`[^`]+`)|(\*\*[^*]+\*\*)|(__[^_]+__)|(\*[^*\n]+\*)|(~~[^~]+~~)|(\[[^\]]*\]\([^)\s]+\))|(https?:\/\/[^\s<>()]+)/g;
const FENCE = /^\s*```(\w*)\s*$/;
const HEADING = /^(#{1,6})\s+(.*)$/;
const RULE = /^\s*(---+|\*\*\*+|___+)\s*$/;
const QUOTE = /^\s*>/;
const TABLE_ROW = /^\s*\|.*\|\s*$/;
const BULLET = /^(\s*)[-*+]\s+(.*)$/;
const NUMBERED = /^(\s*)\d+[.)]\s+(.*)$/;

/** Render the subset of Markdown plans actually use without ever accepting HTML. */
export function BoardMarkdown({
  source,
  tight = false,
}: {
  source: string;
  tight?: boolean;
}) {
  return <div className={`board-md${tight ? " tight" : ""}`}>{blocks(source)}</div>;
}

function blocks(source: string): ReactNode[] {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const result: ReactNode[] = [];
  let index = 0;
  let key = 0;

  while (index < lines.length) {
    const line = lines[index] ?? "";
    if (line.trim() === "") {
      index += 1;
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence) {
      const body: string[] = [];
      index += 1;
      while (index < lines.length && !/^\s*```/.test(lines[index] ?? "")) {
        body.push(lines[index] ?? "");
        index += 1;
      }
      if (index < lines.length) index += 1;
      result.push(
        <pre key={key++} data-lang={fence[1] || undefined}>
          <code>{body.join("\n")}</code>
        </pre>,
      );
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      const Tag = `h${Math.min((heading[1] ?? "#").length + 2, 6)}` as "h3";
      result.push(<Tag key={key++}>{inline(heading[2] ?? "", `h${key}`)}</Tag>);
      index += 1;
      continue;
    }

    if (RULE.test(line)) {
      result.push(<hr key={key++} />);
      index += 1;
      continue;
    }

    if (QUOTE.test(line)) {
      const body: string[] = [];
      while (index < lines.length && QUOTE.test(lines[index] ?? "")) {
        body.push((lines[index] ?? "").replace(/^\s*>\s?/, ""));
        index += 1;
      }
      result.push(<blockquote key={key++}>{blocks(body.join("\n"))}</blockquote>);
      continue;
    }

    if (TABLE_ROW.test(line)) {
      const rows: string[] = [];
      while (index < lines.length && TABLE_ROW.test(lines[index] ?? "")) {
        rows.push(lines[index] ?? "");
        index += 1;
      }
      result.push(<MarkdownTable key={key++} rows={rows} />);
      continue;
    }

    if (BULLET.test(line) || NUMBERED.test(line)) {
      const ordered = NUMBERED.test(line) && !BULLET.test(line);
      const items: string[] = [];
      while (index < lines.length) {
        const current = lines[index] ?? "";
        const item = BULLET.exec(current) ?? NUMBERED.exec(current);
        if (item) {
          items.push(item[2] ?? "");
          index += 1;
        } else if (/^\s{2,}\S/.test(current) && items.length > 0) {
          items[items.length - 1] += ` ${current.trim()}`;
          index += 1;
        } else {
          break;
        }
      }
      const List = ordered ? "ol" : "ul";
      result.push(
        <List key={key++}>
          {items.map((item, itemIndex) => (
            <li key={itemIndex}>{inline(item, `li${itemIndex}`)}</li>
          ))}
        </List>,
      );
      continue;
    }

    const paragraph: string[] = [];
    while (index < lines.length && !startsBlock(lines[index] ?? "")) {
      paragraph.push(lines[index] ?? "");
      index += 1;
    }
    // Keep an unfamiliar line visible and guarantee forward progress.
    if (paragraph.length === 0) {
      paragraph.push(line);
      index += 1;
    }
    result.push(<p key={key++}>{inline(paragraph.join(" "), `p${key}`)}</p>);
  }

  return result;
}

function inline(text: string, prefix: string): ReactNode[] {
  const result: ReactNode[] = [];
  let last = 0;
  let match: RegExpExecArray | null;
  let count = 0;
  INLINE.lastIndex = 0;

  while ((match = INLINE.exec(text)) !== null) {
    if (match.index > last) result.push(text.slice(last, match.index));
    const token = match[0];
    const key = `${prefix}-${count++}`;
    if (token.startsWith("`")) {
      result.push(<code key={key}>{token.slice(1, -1)}</code>);
    } else if (token.startsWith("**") || token.startsWith("__")) {
      result.push(<strong key={key}>{token.slice(2, -2)}</strong>);
    } else if (token.startsWith("~~")) {
      result.push(<del key={key}>{token.slice(2, -2)}</del>);
    } else if (token.startsWith("*")) {
      result.push(<em key={key}>{token.slice(1, -1)}</em>);
    } else if (token.startsWith("[")) {
      const split = token.indexOf("](");
      const label = token.slice(1, split);
      const href = safeHref(token.slice(split + 2, -1));
      result.push(
        href ? (
          <a key={key} href={href} target="_blank" rel="noreferrer noopener">
            {label}
          </a>
        ) : (
          <span key={key}>{label}</span>
        ),
      );
    } else {
      result.push(
        <a key={key} href={token} target="_blank" rel="noreferrer noopener">
          {token}
        </a>,
      );
    }
    last = match.index + token.length;
  }
  if (last < text.length) result.push(text.slice(last));
  return result;
}

function MarkdownTable({ rows }: { rows: string[] }) {
  const cells = (row: string) =>
    row.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((cell) => cell.trim());
  const [head, ...rest] = rows;
  const body = rest.filter((row) => !/^\s*\|[\s:|-]+\|\s*$/.test(row));
  return (
    <table>
      {head && (
        <thead>
          <tr>{cells(head).map((cell, index) => <th key={index}>{inline(cell, `th${index}`)}</th>)}</tr>
        </thead>
      )}
      <tbody>
        {body.map((row, rowIndex) => (
          <tr key={rowIndex}>
            {cells(row).map((cell, index) => (
              <td key={index}>{inline(cell, `td${rowIndex}-${index}`)}</td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function startsBlock(line: string): boolean {
  return (
    line.trim() === "" ||
    FENCE.test(line) ||
    HEADING.test(line) ||
    RULE.test(line) ||
    QUOTE.test(line) ||
    TABLE_ROW.test(line) ||
    BULLET.test(line) ||
    NUMBERED.test(line)
  );
}

function safeHref(href: string): string | undefined {
  const trimmed = href.trim();
  if (/^(https?:|mailto:)/i.test(trimmed)) return trimmed;
  if (trimmed.startsWith("/") || trimmed.startsWith("#")) return trimmed;
  return undefined;
}
