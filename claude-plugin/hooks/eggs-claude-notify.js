#!/usr/bin/env node
// Claude Code -> eggs desktop pet bridge (cross-platform)
//
// Usage: node eggs-notify.js <event-type>
// Reads hook JSON from stdin, extracts relevant fields, calls eggs CLI.

const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const event = process.argv[2];
if (!event) process.exit(0);

const projectDir = process.env.CLAUDE_PROJECT_DIR || process.cwd();
const homeDir = process.env.HOME || process.env.USERPROFILE;

// Priority order for finding eggs binary:
// 1. EGGS_BIN env var (explicit override)
// 2. Project debug build (if we're in the eggs repo itself)
// 3. ~/.eggs/bin/eggs (standard installation)
// 4. System paths
let defaultEggsBin;

const debugBuild = path.join(projectDir, 'desktop/src-tauri/target/debug/eggs');
const installedBin = path.join(homeDir, '.eggs/bin/eggs');

if (fs.existsSync(debugBuild)) {
  // We're in the eggs development project - use debug build
  defaultEggsBin = debugBuild;
} else if (fs.existsSync(installedBin)) {
  // Standard installation location
  defaultEggsBin = installedBin;
} else {
  // Try common system paths
  const fallbacks = [
    '/usr/local/bin/eggs',
    path.join(homeDir, 'bin/eggs'),
    'eggs'  // Let shell resolve it
  ];
  defaultEggsBin = fallbacks.find(p => {
    try {
      return fs.existsSync(p);
    } catch {
      return false;
    }
  }) || installedBin;  // Fallback to standard location even if not exists
}

const eggsBin = process.env.EGGS_BIN || defaultEggsBin;

// Check if eggs binary exists
if (!fs.existsSync(eggsBin)) process.exit(0);

// Read JSON payload from stdin
let payload = '';
try {
  payload = fs.readFileSync(0, 'utf-8');
} catch {
  process.exit(0);
}

let data;
try {
  data = JSON.parse(payload);
} catch {
  process.exit(0);
}

function field(path) {
  const keys = path.split('.');
  let val = data;
  for (const key of keys) {
    if (val && typeof val === 'object') val = val[key];
    else return '';
  }
  return val || '';
}

function clip(text) {
  return String(text).replace(/\n/g, ' ').replace(/\s+/g, ' ').slice(0, 80).trim();
}

function send(text) {
  if (!text) return;
  try {
    execSync(`"${eggsBin}" hook "${text.replace(/"/g, '\\"')}"`, {
      stdio: 'ignore',
      windowsHide: true
    });
  } catch {
    // Silently ignore errors
  }
}

switch (event) {
  case 'session-start': {
    const src = field('source') || 'start';
    send(`Agent: session ${src}`);
    break;
  }
  case 'stop': {
    const mode = field('permission_mode');
    const effort = field('effort.level');
    const summary = String(field('last_assistant_message')).replace(/\n/g, ' ').trim();

    if (summary) {
      send(`Agent: ${summary}`);
    }

    let detail = '';
    if (mode) detail += `mode=${mode} `;
    if (effort) detail += `effort=${effort}`;
    detail = detail.trim();

    send(detail ? `Agent: done (${detail})` : 'Agent: done');
    break;
  }
  case 'subagent-stop':
    send('Agent: subagent done');
    break;
  case 'prompt-submit': {
    const txt = (field('prompt'));
    send(`You: ${txt}`);
    break;
  }
  case 'notification': {
    const txt = clip(field('message'));
    send(`Permission: ${txt}`);
    break;
  }
  case 'tool-bash': {
    const txt = clip(field('tool_input.command'));
    send(`Run command: ${txt}`);
    break;
  }
  case 'tool-read': {
    const txt = clip(field('tool_input.file_path'));
    send(`Read file: ${txt}`);
    break;
  }
  case 'tool-write': {
    const txt = clip(field('tool_input.file_path'));
    send(`Write file: ${txt}`);
    break;
  }
  case 'tool-search': {
    const txt = clip(field('tool_input.pattern') || field('tool_input.query'));
    send(`Search: ${txt}`);
    break;
  }
  case 'tool-web-search': {
    const txt = clip(field('tool_input.query'));
    send(`Web search: ${txt}`);
    break;
  }
  case 'tool-web-fetch': {
    const txt = clip(field('tool_input.url'));
    send(`Web fetch: ${txt}`);
    break;
  }
  case 'tool-mcp': {
    const txt = clip(field('tool_name'));
    send(`MCP tool: ${txt}`);
    break;
  }
}
