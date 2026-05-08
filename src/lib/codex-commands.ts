export type CodexCommandSource = "upstream" | "webui";
export type CodexCommandSupport =
  | "native-webui"
  | "app-server-proxied"
  | "ui-only"
  | "not-applicable"
  | "blocked"
  | "planned";

export type CodexSlashCommandEntry = {
  command: string;
  description: string;
  support: CodexCommandSupport;
  source: CodexCommandSource;
  inlineArgs: boolean;
  visibleInComposer: boolean;
};

export const CODEX_SLASH_COMMANDS: CodexSlashCommandEntry[] = [
  { command: "queue", description: "Queue a follow-up message without switching modes.", support: "native-webui", source: "webui", inlineArgs: true, visibleInComposer: true },
  { command: "steer", description: "Steer the active turn immediately.", support: "native-webui", source: "webui", inlineArgs: true, visibleInComposer: true },
  { command: "preset", description: "Load a saved prompt preset into the composer.", support: "native-webui", source: "webui", inlineArgs: true, visibleInComposer: true },
  { command: "model", description: "Choose what model and reasoning effort to use.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "fast", description: "Toggle Fast mode.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "ide", description: "Include current IDE context.", support: "planned", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "permissions", description: "Choose what Codex is allowed to do.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "keymap", description: "Remap TUI shortcuts.", support: "not-applicable", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "vim", description: "Toggle Vim mode for the composer.", support: "not-applicable", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "setup-default-sandbox", description: "Set up elevated agent sandbox.", support: "blocked", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "sandbox-add-read-dir", description: "Let sandbox read a directory.", support: "blocked", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "experimental", description: "Toggle experimental features.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "approve", description: "Approve one retry of a recent auto-review denial.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "memories", description: "Configure memory use and generation.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "skills", description: "Use skills to improve how Codex performs specific tasks.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "hooks", description: "View and manage lifecycle hooks.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "review", description: "Review current changes and find issues.", support: "app-server-proxied", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "rename", description: "Rename the current thread.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "new", description: "Start a new chat during a conversation.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "resume", description: "Resume a saved chat.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "fork", description: "Fork the current chat.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "init", description: "Create an AGENTS.md file with instructions for Codex.", support: "app-server-proxied", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "compact", description: "Summarize conversation to prevent hitting the context limit.", support: "app-server-proxied", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "plan", description: "Switch to Plan mode.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "goal", description: "Set or view the goal for a long-running task.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "collab", description: "Change collaboration mode.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "agent", description: "Switch the active agent thread.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "side", description: "Start a side conversation in an ephemeral fork.", support: "planned", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "copy", description: "Copy last response as markdown.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "raw", description: "Toggle raw scrollback mode for copy-friendly terminal selection.", support: "not-applicable", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "diff", description: "Show git diff.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "mention", description: "Mention a file.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "status", description: "Show current session configuration and token usage.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "debug-config", description: "Show config layers and requirement sources.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "title", description: "Configure terminal title items.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "statusline", description: "Configure terminal status line items.", support: "not-applicable", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "theme", description: "Choose a syntax highlighting theme.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "mcp", description: "List configured MCP tools.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "apps", description: "Manage apps.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "plugins", description: "Browse plugins.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "logout", description: "Log out of Codex.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "quit", description: "Exit Codex.", support: "not-applicable", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "exit", description: "Exit Codex.", support: "not-applicable", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "feedback", description: "Send logs to maintainers.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "rollout", description: "Print the rollout file path.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "ps", description: "List background terminals.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "stop", description: "Stop all background terminals.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "clear", description: "Clear the terminal and start a new chat.", support: "native-webui", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "personality", description: "Choose a communication style for Codex.", support: "native-webui", source: "upstream", inlineArgs: true, visibleInComposer: true },
  { command: "realtime", description: "Toggle realtime voice mode.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "settings", description: "Configure realtime microphone and speaker.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "test-approval", description: "Test approval request.", support: "blocked", source: "upstream", inlineArgs: false, visibleInComposer: false },
  { command: "subagents", description: "Switch the active agent thread.", support: "planned", source: "upstream", inlineArgs: false, visibleInComposer: true },
  { command: "debug-m-drop", description: "Debug memory drop command.", support: "blocked", source: "upstream", inlineArgs: false, visibleInComposer: false },
  { command: "debug-m-update", description: "Debug memory update command.", support: "blocked", source: "upstream", inlineArgs: false, visibleInComposer: false }
];

export function findCodexSlashCommand(command: string) {
  const normalized = command.trim().replace(/^\//u, "").toLowerCase();
  return CODEX_SLASH_COMMANDS.find((entry) => entry.command === normalized) ?? null;
}
