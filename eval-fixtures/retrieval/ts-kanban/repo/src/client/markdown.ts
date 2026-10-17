/**
 * Small Markdown subset for card descriptions: headings, emphasis, inline
 * code, fenced code, links, bullet lists, and task checkboxes. Output is
 * passed through sanitize.ts before it touches the DOM.
 */
import { cleanHtml } from "./sanitize";

function escape(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function inline(src: string): string {
  let s = escape(src);
  s = s.replace(/`([^`]+)`/g, "<code>$1</code>");
  s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/\*([^*]+)\*/g, "<em>$1</em>");
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, '<a href="$2">$1</a>');
  return s;
}

export function renderMarkdown(src: string): string {
  const out: string[] = [];
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  let inList = false;
  let inFence = false;
  let fence: string[] = [];

  const closeList = () => {
    if (inList) out.push("</ul>");
    inList = false;
  };

  for (const line of lines) {
    if (line.startsWith("```")) {
      if (inFence) {
        out.push(`<pre><code>${escape(fence.join("\n"))}</code></pre>`);
        fence = [];
      } else {
        closeList();
      }
      inFence = !inFence;
      continue;
    }
    if (inFence) {
      fence.push(line);
      continue;
    }
    const heading = /^(#{1,4})\s+(.*)$/.exec(line);
    const task = /^[-*]\s+\[( |x)\]\s+(.*)$/i.exec(line);
    const bullet = /^[-*]\s+(.*)$/.exec(line);
    if (heading) {
      closeList();
      const level = heading[1].length + 2; // h3..h6 inside a card
      out.push(`<h${level}>${inline(heading[2])}</h${level}>`);
    } else if (task || bullet) {
      if (!inList) out.push("<ul>");
      inList = true;
      out.push(
        task
          ? `<li class="task"><input type="checkbox" disabled${task[1] !== " " ? " checked" : ""}> ${inline(task[2])}</li>`
          : `<li>${inline(bullet![1])}</li>`,
      );
    } else if (line.trim() === "") {
      closeList();
    } else {
      closeList();
      out.push(`<p>${inline(line)}</p>`);
    }
  }
  closeList();
  return cleanHtml(out.join("\n"));
}
