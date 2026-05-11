#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

const READ_COMMANDS = new Set([
  "cat",
  "head",
  "tail",
  "less",
  "more",
  "sed",
  "nl",
  "wc",
  "ls",
  "find",
  "pwd",
  "tree",
]);
const SEARCH_COMMANDS = new Set(["rg", "grep", "ag", "ack"]);
const WRITE_COMMANDS = new Set([
  "apply_patch",
  "tee",
  "touch",
  "mkdir",
  "cp",
  "mv",
  "install",
]);

function readPayload() {
  try {
    const raw = fs.readFileSync(0, "utf-8");
    if (!raw.trim()) return {};
    const payload = JSON.parse(raw);
    return typeof payload === "object" && payload !== null ? payload : {};
  } catch {
    return {};
  }
}

function shorten(text, limit = 110) {
  const compact = (text || "").replace(/\s+/g, " ").trim();
  if (compact.length <= limit) return compact;
  return compact.slice(0, Math.max(0, limit - 1)).trimEnd() + "…";
}

function done() {
  process.stdout.write("{\"continue\": true}\n");
}

function resolveEggsExe() {
  const windows = process.platform === "win32";
  const binaryNames = windows ? ["eggs.exe", "eggs"] : ["eggs", "eggs.exe"];
  const homeDir = process.env.HOME || process.env.USERPROFILE || "";
  const binDir = process.env.EGGS_BIN_DIR || path.join(homeDir, ".eggs", "bin");
  const repoRoot = path.resolve(__dirname, "../../..");
  const localDebug = path.join(repoRoot, "desktop", "src-tauri", "target", "debug");
  const localRelease = path.join(repoRoot, "desktop", "src-tauri", "target", "release");
  const candidates = [];

  for (const name of binaryNames) {
    candidates.push(path.join(localDebug, name));
    candidates.push(path.join(binDir, name));
    candidates.push(path.join(localRelease, name));
  }

  for (const candidate of candidates) {
    if (fs.existsSync(candidate) && isExecutable(candidate)) {
      return candidate;
    }
  }

  for (const name of binaryNames) {
    try {
      const which = process.platform === "win32" ? "where" : "which";
      const result = execFileSync(which, [name], {
        encoding: "utf-8",
        stdio: ["ignore", "pipe", "ignore"],
      });
      const onPath = result.trim().split("\n")[0];
      if (onPath && fs.existsSync(onPath)) return onPath;
    } catch {
      // Ignore PATH lookup failure.
    }
  }

  return null;
}

function isExecutable(filePath) {
  try {
    if (process.platform === "win32") {
      return fs.existsSync(filePath) && fs.statSync(filePath).isFile();
    }
    const stats = fs.statSync(filePath);
    return stats.isFile() && (stats.mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

function writeBubbleSpool(text) {
  const homeDir = process.env.HOME || process.env.USERPROFILE || "";
  const appDir = path.join(homeDir, ".eggs");
  const spoolDir = path.join(appDir, "bubble-spool");
  const eventId = `hook-${Date.now().toString(16)}-${Math.random().toString(16).slice(2, 10)}`;
  const payload = {
    id: eventId,
    owner: { kind: "local" },
    source: "hook",
    text,
    ttl_ms: 8000,
    created_ms: Date.now(),
    room_code: null,
    device_id: null,
  };

  try {
    fs.mkdirSync(spoolDir, { recursive: true });
    const tmpFile = path.join(spoolDir, `${eventId}.tmp`);
    const finalFile = path.join(spoolDir, `${eventId}.json`);
    fs.writeFileSync(tmpFile, JSON.stringify(payload), "utf-8");
    fs.renameSync(tmpFile, finalFile);
  } catch {
    // Ignore spool write failure.
  }
}

function sendHook(text) {
  if (!text) return;
  const exe = resolveEggsExe();
  if (exe) {
    try {
      execFileSync(exe, ["hook", text], {
        stdio: ["ignore", "ignore", "ignore"],
        windowsHide: true,
      });
      return;
    } catch {
      // Fall through to spool.
    }
  }
  writeBubbleSpool(text);
}

function firstString(value) {
  if (typeof value === "string") return value;
  if (typeof value === "object" && value !== null) {
    for (const key of ["cmd", "command", "query", "url", "path", "file_path"]) {
      const item = value[key];
      if (typeof item === "string" && item.trim()) return item;
    }
  }
  return "";
}

function toolInput(payload) {
  for (const key of ["tool_input", "input", "arguments", "args", "parameters"]) {
    const value = payload[key];
    if (value) return value;
  }
  return {};
}

function commandFromPayload(payload) {
  const data = toolInput(payload);
  if (typeof data === "object" && data !== null) {
    for (const key of ["cmd", "command", "script", "query", "url"]) {
      const value = data[key];
      if (typeof value === "string" && value.trim()) return value;
    }
  }
  return firstString(data);
}

function commandHead(command) {
  let parts;
  try {
    parts = command.match(/(?:[^\s"]+|"[^"]*")+/g) || [];
    parts = parts.map((part) => part.replace(/^"|"$/g, ""));
  } catch {
    parts = command.split(/\s+/);
  }

  if (!parts.length) return "";
  let head = parts[0].split("/").pop();
  if (["env", "command", "xargs"].includes(head) && parts.length > 1) {
    head = parts[1].split("/").pop();
  }
  return head;
}

function labelForTool(tool, command) {
  const normalized = (tool || "").trim();
  const lower = normalized.toLowerCase();
  const cmd = commandHead(command).toLowerCase();

  if (["apply_patch", "edit", "write", "multiedit"].includes(lower)) return "Write file";
  if (["read", "glob", "list", "ls"].includes(lower)) return "Read file";
  if (["grep", "search"].includes(lower)) return "Search";
  if (lower.includes("web_search") || lower.endsWith("websearch")) return "Web search";
  if (lower.includes("web_fetch") || lower.endsWith("webfetch")) return "Web fetch";
  if (lower.startsWith("mcp__")) {
    if (lower.includes("search")) return "MCP search";
    if (lower.includes("fetch") || lower.includes("read")) return "MCP fetch";
    return "MCP tool";
  }

  if (SEARCH_COMMANDS.has(cmd)) return "Search";
  if (READ_COMMANDS.has(cmd)) return "Read file";
  if (WRITE_COMMANDS.has(cmd)) return "Write file";
  if (["curl", "wget", "http", "https"].includes(cmd)) return "Web fetch";
  if (["Bash", "exec_command"].includes(normalized) || cmd) return "Run command";
  return "Tool done";
}

function detailForTool(tool, command) {
  if (command) return shorten(command, 80);
  if (tool) return tool;
  return "tool";
}

function formatHookMessage(payload) {
  const tool = payload.tool_name || payload.tool || payload.name || "tool";
  const command = commandFromPayload(payload);
  const label = labelForTool(tool, command);
  const detail = detailForTool(tool, command);
  return `${label}: ${detail}`;
}

function reasonFromPayload(payload) {
  for (const key of ["reason", "justification", "message", "description"]) {
    const value = payload[key];
    if (typeof value === "string" && value.trim()) {
      return shorten(value, 90);
    }
  }
  return "";
}

function handleUserPromptSubmit(payload) {
  const prompt = payload.prompt || payload.user_prompt || payload.input || "";
  sendHook(`User: ${prompt}`);
}

function handlePostToolUse(payload) {
  sendHook(formatHookMessage(payload));
}

function handlePermissionRequest(payload) {
  const tool = payload.tool_name || payload.tool || payload.name || "unknown";
  const command = commandFromPayload(payload);
  const label = labelForTool(tool, command);
  const detail = detailForTool(tool, command);
  const reason = reasonFromPayload(payload);
  let message = `Permission: ${label}: ${detail}`;
  if (reason) message = `${message} | ${reason}`;
  sendHook(message);
}

function handleStop(payload) {
  const text = payload.last_assistant_message || payload.assistant_message || "";
  if (text) sendHook(`Agent: ${text}`);
}

function main() {
  const hookType = process.argv[2] || process.env.CODEX_HOOK_TYPE || "";
  const payload = readPayload();

  switch (hookType) {
    case "user_prompt_submit":
      handleUserPromptSubmit(payload);
      break;
    case "post_tool_use":
      handlePostToolUse(payload);
      break;
    case "permission_request":
      handlePermissionRequest(payload);
      break;
    case "stop":
      handleStop(payload);
      break;
    default:
      if (payload.prompt || payload.user_prompt || payload.input) {
        handleUserPromptSubmit(payload);
      } else if (payload.tool_name || payload.tool || payload.name) {
        handlePostToolUse(payload);
      }
      break;
  }

  done();
}

if (require.main === module) {
  main();
}
