/**
 * 收尾信封 → **人话**（纯逻辑，不碰 React、不碰 Tauri，改完能单独验算）。
 *
 * 一轮跑着的时候，`llm_delta` 的 `content` 通道给的就是模型原文——也就是那串收尾
 * 信封 JSON（`{"summary": "…", "findings": […]}`）。于是"正在写的正文"在人眼里
 * 是一段带大括号的机器字：换行是两个**字面字符** `\` + `n`，Markdown 的列表 / 标题 /
 * 表格全挤成一整行，`findings` 数组也跟在后面。收尾之后才由 sidecar 解析出人话
 * （`provider._salvage_summary()`），界面这才干净——同一段正文，前后两副面孔
 * （《GUI 待修问题 2026-09-24》）。
 *
 * 这里把服务端那一套搬到客户端，**流到一半也照样给到当前进度**：
 *
 * * 取 `summary` / `answer` / `text` 字段的值（模型常用的三个别名）；
 * * 值是 JSON 字符串时按字符串读到收尾引号（字符串还没闭合=正在流，就给到当前）；
 * * 解掉 JSON 转义（`\n` / `\t` / `\"` / `\\` / `\uXXXX`），换行恢复成真换行；
 * * **只在文本以信封开头时才动手**——普通散文一个字都不动（和服务端同一条判据，
 *   也免得把一段正常回答从中间截断）；
 * * 信封只写了个开头（`{"summ`）时返回空串：此时没有人话可给，界面照旧显示
 *   "正在追查…"，不把半截 JSON 画到结论卡上。
 */

/** 字段名允许被 `"` / `[` / `{` 包着，也允许中文冒号（模型实测写法，见 §5.1.1 第 32 条）。 */
const SUMMARY_FIELD = /^(?:[{[]+\s*)?["[{]?\s*(?:summary|answer|text)\s*["\]}]?\s*[:：]\s*/i;
/** 信封把机器字段写在前面时（`{"findings": […], "summary": "…"}`）：顶层对象的键。 */
const TOP_QUOTED_FIELD = /^"(?:summary|answer|text)"\s*[:：]\s*/i;
const TOP_BARE_FIELD = /^(?:summary|answer|text)\s*[:：]\s*/i;
/** `findings` 的位置：它之后都是机器字段，不属于"给人看的那段话"（无引号信封用）。 */
const FINDINGS_FIELD = /["[{]?\s*findings\s*["\]}]?\s*[:：]/i;
/** ```json 这层围栏（模型爱套），开头的围栏不该出现在正文里。 */
const OPENING_FENCE = /^```[A-Za-z0-9_-]*[ \t]*\r?\n/;
/** 信封里除人话之外还认得的字段名——用它们判断"这串还在信封开头"。 */
const FIELD_NAMES = ["summary", "answer", "text", "findings"];

/** 解 JSON 字符串里的转义；认不出来的反斜杠原样留着。 */
function decodeEscapes(text: string): string {
  const simple: Record<string, string> = {
    n: "\n",
    r: "\r",
    t: "\t",
    b: "\b",
    f: "\f",
    '"': '"',
    "\\": "\\",
    "/": "/",
  };
  let out = "";
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char !== "\\") {
      out += char;
      continue;
    }
    const next = text[index + 1];
    // 半个转义（正好流到 `\`）先留着：下一帧到齐了自然会对上。
    if (next === undefined) {
      out += char;
      break;
    }
    if (next === "u") {
      const hex = text.slice(index + 2, index + 6);
      if (/^[0-9a-fA-F]{4}$/.test(hex)) {
        out += String.fromCharCode(Number.parseInt(hex, 16));
        index += 5;
        continue;
      }
    } else if (next in simple) {
      out += simple[next];
      index += 1;
      continue;
    }
    out += char;
  }
  return out;
}

/** 从 JSON 字符串（`"` 开头）里读到收尾引号；没闭合就返回当前已有的全部。 */
function readString(source: string): string {
  let out = "";
  for (let index = 1; index < source.length; index += 1) {
    const char = source[index];
    if (char === "\\") {
      const next = source[index + 1];
      if (next === undefined) {
        out += char;
        break;
      }
      out += char + next;
      index += 1;
      continue;
    }
    if (char === '"') return out;
    out += char;
  }
  return out;
}

/**
 * 这串是不是"信封（还没写完）"？
 *
 * 是的话现在没有人话可给——`{"summ` 看着像字段名的一半，`{"findings": [` 则是
 * 这一轮根本没写 `summary`（纯工具轮）。判据只在**开头**几个字符上看，绝不误伤散文。
 */
function looksLikeEnvelope(head: string): boolean {
  const rest = head.replace(/^[{["'\s]+/, "").toLowerCase();
  if (!rest) return true;
  return FIELD_NAMES.some((name) => name.startsWith(rest) || rest.startsWith(name));
}

/**
 * 信封把机器字段写在前面时的兜底：在**顶层**（深度 1）找 `summary` / `answer` / `text`
 * 的键，返回它之后的起点；找不到返回 -1。
 *
 * 为什么不能直接全文搜：`findings` 数组里**每条结论自己也带一个 `summary`**
 * （深度 2），全文搜会把某条结论的解释当成"这一段回答"，字还会随着新结论往下长。
 * 这里按字符串与括号深度走一遍，只认顶层那个键。
 */
function topLevelFieldEnd(text: string): number {
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (char === "\\") escaped = true;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') {
      if (depth === 1) {
        const quoted = TOP_QUOTED_FIELD.exec(text.slice(index));
        if (quoted) return index + quoted[0].length;
      }
      inString = true;
      continue;
    }
    if (depth === 1 && !/[{[]/.test(char)) {
      const bare = TOP_BARE_FIELD.exec(text.slice(index));
      if (bare) return index + bare[0].length;
    }
    if (char === "{" || char === "[") depth += 1;
    else if (char === "}" || char === "]") depth -= 1;
  }
  return -1;
}

/**
 * 模型原文 → 给人看的那段话（可能还没写完）。
 *
 * 返回空串表示"还没有人话可给"（信封只开了个头 / 本轮只有 `findings`）。
 */
export function humanDraft(raw: string): string {
  const unfenced = raw.replace(OPENING_FENCE, "");
  const head = unfenced.replace(/^\s+/, "");
  if (!head) return "";

  let start = SUMMARY_FIELD.exec(head)?.[0].length ?? -1;
  if (start < 0) {
    if (!looksLikeEnvelope(head)) {
      // 不像信封：普通散文（含模型先说话、再给信封的情形）原样返回。
      return unfenced.trim();
    }
    // 是信封、但人话不在第一个字段：顶层找一遍。找不到就是"还没写到"（信封才开了
    // 个头）或"这一轮根本没写人话"（只有 findings）——两种都返回空串。
    start = topLevelFieldEnd(head);
    if (start < 0) return "";
  }

  const body = head.slice(start);
  if (body.startsWith('"')) return decodeEscapes(readString(body)).trim();
  if (body.startsWith("「")) {
    const end = body.indexOf("」", 1);
    return body.slice(1, end < 0 ? body.length : end).trim();
  }

  // 无引号的散文式信封（`[summary]: 从工具结果看…`）：截到 findings 之前。
  const cut = FINDINGS_FIELD.exec(body);
  const value = (cut ? body.slice(0, cut.index) : body).trim();
  return decodeEscapes(value.replace(/[,\s}\]]+$/, "")).trim();
}
